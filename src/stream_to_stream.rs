use futures::StreamExt;
use futures::stream::BoxStream;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_stream::Stream;

type Result<T> = std::result::Result<T, ToolCallStreamError>;

/// ストリーム処理中に発生する可能性のあるエラー
#[derive(thiserror::Error, Debug)]
pub enum ToolCallStreamError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Unexpected end of file")]
    UnexpectedEof,
}

/// ストリーミングイベントを表すenum
/// XMLの解析結果を表現するために使用される
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallEvent {
    /// テキストイベント：XMLタグ以外のテキストを表す
    Text(String),
    /// ツール呼び出しの開始：<tool_name>タグの検出
    ToolStart { name: String },
    /// パラメータの受信：ツール呼び出しに含まれるパラメータ
    Parameter { arguments: serde_json::Value },
    /// ツール呼び出しの終了：</tool_name>タグの検出
    ToolEnd,
    /// エラーイベント：処理中に発生したエラー
    Error(String),
}

type ToolCallStream = Result<BoxStream<'static, ToolCallEvent>>;

/// パーサーの状態を表すenum
#[derive(Debug)]
enum ParserState {
    /// 通常状態：XMLタグ外
    Normal,
    /// タグ解析中：< と > の間
    InTag,
    /// ツールタグ内：<tool_name> と </tool_name> の間
    InToolTag,
    /// パラメータタグ内：<param_name> と </param_name> の間
    InParameterTag(String), // param_name
}

/// XMLストリームをイベントストリームに変換するための構造体
struct StreamToStream {
    /// 入力ストリーム
    input: BoxStream<'static, String>,
    /// タグ名を一時的に保存するバッファ
    tag_buffer: String,
    /// 現在のパーサー状態
    state: ParserState,
    /// 現在のツールのパラメータを保持
    current_params: serde_json::Map<String, serde_json::Value>,
    /// パラメータの値を一時的に保存するバッファ
    param_value_buffer: String,
    /// 現在処理中のツール名
    current_tool: Option<String>,
    /// 直前の文字が改行だったかどうか
    last_char_was_newline: bool,
    /// ToolEndイベントを発行する必要があるかどうか
    need_to_emit_tool_end: bool,
    /// XMLタグ内にいるかどうか
    in_xml: bool,
}

impl StreamToStream {
    /// 新しいStreamToStreamインスタンスを作成
    fn new(input: BoxStream<'static, String>) -> Self {
        Self {
            input,
            tag_buffer: String::new(),
            state: ParserState::Normal,
            current_params: serde_json::Map::new(),
            param_value_buffer: String::new(),
            current_tool: None,
            last_char_was_newline: false,
            need_to_emit_tool_end: false,
            in_xml: false,
        }
    }

    /// 1文字を処理し、必要に応じてイベントを生成
    ///
    /// # 改行の処理ルール
    /// - XMLタグ内の改行は無視
    /// - 連続する改行は1回だけ出力
    /// - それ以外の改行は通常通り出力
    fn process_char(&mut self, c: &str) -> Option<ToolCallEvent> {
        match &self.state {
            ParserState::Normal => {
                if c == "<" {
                    // XMLタグの開始
                    self.state = ParserState::InTag;
                    self.tag_buffer.clear();
                    self.in_xml = true;
                    self.last_char_was_newline = false; // XMLタグ開始時にリセット
                    None
                } else {
                    if c == "\n" {
                        if self.in_xml {
                            // XMLタグ内の改行は無視
                            None
                        } else if self.last_char_was_newline {
                            // 連続する改行は無視
                            None
                        } else {
                            // 最初の改行は出力
                            self.last_char_was_newline = true;
                            Some(ToolCallEvent::Text(c.to_string()))
                        }
                    } else if !c.trim().is_empty() {
                        // 通常のテキスト
                        self.last_char_was_newline = false;
                        Some(ToolCallEvent::Text(c.to_string()))
                    } else if self.in_xml {
                        // XMLタグ内の空白は無視
                        None
                    } else {
                        // その他の空白は無視
                        None
                    }
                }
            }
            ParserState::InTag => {
                if c == ">" {
                    // タグの終了
                    let tag = std::mem::take(&mut self.tag_buffer);
                    if tag.starts_with('/') {
                        // 終了タグの処理
                        let tag_name = tag[1..].to_string();
                        if let Some(current_tool) = &self.current_tool {
                            if current_tool == &tag_name {
                                // ツールの終了
                                self.state = ParserState::Normal;
                                self.current_tool = None;
                                self.in_xml = false;
                                self.last_char_was_newline = false;
                                if !self.current_params.is_empty() {
                                    // パラメータがある場合は、まずパラメータを出力
                                    self.need_to_emit_tool_end = true;
                                    Some(ToolCallEvent::Parameter {
                                        arguments: serde_json::Value::Object(
                                            self.current_params.clone(),
                                        ),
                                    })
                                } else {
                                    // パラメータがない場合は、直接ToolEndを出力
                                    Some(ToolCallEvent::ToolEnd)
                                }
                            } else {
                                // パラメータタグの終了
                                let value = std::mem::take(&mut self.param_value_buffer);
                                self.current_params.insert(
                                    tag_name,
                                    serde_json::Value::String(value.trim().to_string()),
                                );
                                self.state = ParserState::InToolTag;
                                None
                            }
                        } else {
                            // 予期しない終了タグ
                            self.state = ParserState::Normal;
                            self.in_xml = false;
                            None
                        }
                    } else {
                        if self.current_tool.is_none() {
                            // ツールの開始
                            self.current_tool = Some(tag.clone());
                            self.state = ParserState::InToolTag;
                            Some(ToolCallEvent::ToolStart { name: tag })
                        } else {
                            // パラメータタグの開始
                            self.state = ParserState::InParameterTag(tag);
                            self.param_value_buffer.clear();
                            None
                        }
                    }
                } else {
                    // タグ名の収集
                    self.tag_buffer.push_str(c);
                    None
                }
            }
            ParserState::InToolTag => {
                if c == "<" {
                    // 新しいタグの開始
                    self.state = ParserState::InTag;
                    self.tag_buffer.clear();
                    None
                } else if !c.trim().is_empty() {
                    // ツールタグ内のテキスト
                    Some(ToolCallEvent::Text(c.to_string()))
                } else {
                    None
                }
            }
            ParserState::InParameterTag(_) => {
                if c == "<" {
                    // パラメータタグの終了
                    self.state = ParserState::InTag;
                    self.tag_buffer.clear();
                    None
                } else {
                    // パラメータ値の収集
                    self.param_value_buffer.push_str(c);
                    None
                }
            }
        }
    }
}

/// Stream traitの実装
impl Stream for StreamToStream {
    type Item = ToolCallEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();

        // ToolEndイベントの遅延発行
        if this.need_to_emit_tool_end {
            this.need_to_emit_tool_end = false;
            return Poll::Ready(Some(ToolCallEvent::ToolEnd));
        }

        // 入力ストリームからの次の文字を処理
        match this.input.poll_next_unpin(cx) {
            Poll::Ready(Some(c)) => {
                if let Some(event) = this.process_char(&c) {
                    Poll::Ready(Some(event))
                } else {
                    self.poll_next(cx)
                }
            }
            Poll::Ready(None) => {
                // 入力ストリームの終了処理
                if this.need_to_emit_tool_end {
                    this.need_to_emit_tool_end = false;
                    Poll::Ready(Some(ToolCallEvent::ToolEnd))
                } else if this.current_tool.is_some() {
                    this.current_tool = None;
                    Poll::Ready(Some(ToolCallEvent::ToolEnd))
                } else {
                    Poll::Ready(None)
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// 入力ストリームをツール呼び出しイベントのストリームに変換
fn stream_to_stream(input: BoxStream<'static, String>) -> ToolCallStream {
    let stream = StreamToStream::new(input);
    Ok(Box::pin(stream))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tokio_stream::StreamExt;

    use super::*;

    /// メインのストリーム変換テスト
    ///
    /// このテストでは以下の点を確認します：
    /// - テキストの1文字ずつの処理
    /// - XMLタグの適切な解析
    /// - パラメータの収集と出力
    /// - 改行の適切な処理
    #[tokio::test]
    async fn test_stream_to_stream() -> Result<()> {
        let input = r#"明日のニューヨークの天気ですね。承知いたしました。
        
        <get_weather>
          <location>New York</location>
          <date>tomorrow</date>
          <unit>fahrenheit</unit>
        </get_weather>
        
        結果が取得でき次第、すぐにお知らせします。"#;
        let input_stream = Box::pin(futures::stream::iter(input.chars().map(|c| c.to_string())));

        let expected_events = vec![
            // 一文字ずつ返す
            ToolCallEvent::Text("明".into()),
            ToolCallEvent::Text("日".into()),
            ToolCallEvent::Text("の".into()),
            ToolCallEvent::Text("ニ".into()),
            ToolCallEvent::Text("ュ".into()),
            ToolCallEvent::Text("ー".into()),
            ToolCallEvent::Text("ヨ".into()),
            ToolCallEvent::Text("ー".into()),
            ToolCallEvent::Text("ク".into()),
            ToolCallEvent::Text("の".into()),
            ToolCallEvent::Text("天".into()),
            ToolCallEvent::Text("気".into()),
            ToolCallEvent::Text("で".into()),
            ToolCallEvent::Text("す".into()),
            ToolCallEvent::Text("ね".into()),
            ToolCallEvent::Text("。".into()),
            ToolCallEvent::Text("承".into()),
            ToolCallEvent::Text("知".into()),
            ToolCallEvent::Text("い".into()),
            ToolCallEvent::Text("た".into()),
            ToolCallEvent::Text("し".into()),
            ToolCallEvent::Text("ま".into()),
            ToolCallEvent::Text("し".into()),
            ToolCallEvent::Text("た".into()),
            ToolCallEvent::Text("。".into()),
            ToolCallEvent::Text("\n".into()),
            ToolCallEvent::ToolStart {
                name: "get_weather".to_string(),
            },
            ToolCallEvent::Parameter {
                arguments: serde_json::json!({
                    "location": "New York",
                    "date": "tomorrow",
                    "unit": "fahrenheit"
                }),
            },
            ToolCallEvent::ToolEnd,
            ToolCallEvent::Text("\n".into()),
            ToolCallEvent::Text("結".into()),
            ToolCallEvent::Text("果".into()),
            ToolCallEvent::Text("が".into()),
            ToolCallEvent::Text("取".into()),
            ToolCallEvent::Text("得".into()),
            ToolCallEvent::Text("で".into()),
            ToolCallEvent::Text("き".into()),
            ToolCallEvent::Text("次".into()),
            ToolCallEvent::Text("第".into()),
            ToolCallEvent::Text("、".into()),
            ToolCallEvent::Text("す".into()),
            ToolCallEvent::Text("ぐ".into()),
            ToolCallEvent::Text("に".into()),
            ToolCallEvent::Text("お".into()),
            ToolCallEvent::Text("知".into()),
            ToolCallEvent::Text("ら".into()),
            ToolCallEvent::Text("せ".into()),
            ToolCallEvent::Text("し".into()),
            ToolCallEvent::Text("ま".into()),
            ToolCallEvent::Text("す".into()),
            ToolCallEvent::Text("。".into()),
        ];
        let mut stream = stream_to_stream(input_stream)?;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        assert_eq!(events, expected_events);
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    unimplemented!()
}

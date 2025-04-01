use futures::stream::BoxStream;

type Result<T> = std::result::Result<T, ToolCallStreamError>;

#[derive(thiserror::Error, Debug)]
pub enum ToolCallStreamError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Unexpected end of file")]
    UnexpectedEof,
}

// ストリーミングイベントを表すenum
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallEvent {
    Text(String),
    // ツール呼び出しの開始
    ToolStart { name: String },
    // パラメータの受信
    Parameter { arguments: serde_json::Value },
    // ツール呼び出しの終了
    ToolEnd,
    // エラーイベント
    Error(String),
}

type ToolCallStream = Result<BoxStream<'static, ToolCallEvent>>;

fn stream_to_stream(input: BoxStream<'static, String>) -> ToolCallStream {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use tokio_stream::StreamExt;

    use super::*;

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
            ToolCallEvent::ToolEnd,
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

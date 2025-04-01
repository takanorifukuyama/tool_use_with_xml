use quick_xml::Reader;
use quick_xml::events::Event;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_stream::Stream;
use tokio_stream::StreamExt;

// ストリーミングイベントを表すenum
#[derive(Debug, Clone)]
pub enum ToolCallEvent {
    // ツール呼び出しの開始
    ToolStart(String),
    // パラメータの受信
    Parameter { name: String, value: String },
    // ツール呼び出しの終了
    ToolEnd,
    // エラーイベント
    Error(String),
}

// ストリーミングパーサーの状態を表すenum
#[derive(Debug, Clone)]
enum ParserState {
    Initial,
    InTool(String),
    InParameter { name: String, tool: String },
    ExpectingParameterValue { name: String, tool: String },
}

// パーサーの状態更新を表す構造体
#[derive(Clone)]
struct StateUpdate {
    new_state: ParserState,
    new_tool: Option<String>,
    event: Option<ToolCallEvent>,
}

// ストリーミングパーサー構造体
pub struct ToolCallStream {
    buffer: Vec<u8>,
    position: usize,
    state: ParserState,
    current_tool: Option<String>,
}

#[derive(Debug, Clone)]
pub enum XmlError {
    Xml(String),
    Other(String),
}

impl From<quick_xml::Error> for XmlError {
    fn from(err: quick_xml::Error) -> Self {
        XmlError::Xml(err.to_string())
    }
}

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XmlError::Xml(e) => write!(f, "XML error: {}", e),
            XmlError::Other(e) => write!(f, "Error: {}", e),
        }
    }
}

impl std::error::Error for XmlError {}

impl ToolCallStream {
    pub fn new(initial_data: &[u8]) -> Self {
        Self {
            buffer: initial_data.to_vec(),
            position: 0,
            state: ParserState::Initial,
            current_tool: None,
        }
    }

    pub fn push_data(&mut self, data: &[u8]) {
        if self.position > 0 && self.position == self.buffer.len() {
            self.buffer.clear();
            self.position = 0;
        }
        self.buffer.extend_from_slice(data);
    }

    fn get_reader(&self) -> Reader<&[u8]> {
        let mut reader = Reader::from_reader(&self.buffer[self.position..]);
        reader.trim_text(true);
        reader.check_end_names(false);
        reader
    }

    fn update_position(&mut self, event: &Event) {
        let size = match event {
            Event::Start(e) => e.name().as_ref().len() + 2, // < + name + >
            Event::End(e) => e.name().as_ref().len() + 3,   // </ + name + >
            Event::Text(e) => e.as_ref().len(),
            Event::Eof => 0,
            _ => 1,
        };
        self.position += size;
    }

    fn process_event(&self, event: &Event, state: &ParserState) -> StateUpdate {
        match (state, event) {
            (ParserState::Initial, Event::Start(e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                StateUpdate {
                    new_state: ParserState::InTool(tag_name.clone()),
                    new_tool: Some(tag_name.clone()),
                    event: Some(ToolCallEvent::ToolStart(tag_name)),
                }
            }
            (ParserState::InTool(tool_name), Event::Start(e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                StateUpdate {
                    new_state: ParserState::InParameter {
                        name: tag_name.clone(),
                        tool: tool_name.clone(),
                    },
                    new_tool: self.current_tool.clone(),
                    event: None,
                }
            }
            (ParserState::InParameter { name, tool }, Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().trim().to_string();
                if !text.is_empty() {
                    StateUpdate {
                        new_state: ParserState::InParameter {
                            name: name.clone(),
                            tool: tool.clone(),
                        },
                        new_tool: self.current_tool.clone(),
                        event: Some(ToolCallEvent::Parameter {
                            name: name.clone(),
                            value: text,
                        }),
                    }
                } else {
                    StateUpdate {
                        new_state: state.clone(),
                        new_tool: self.current_tool.clone(),
                        event: None,
                    }
                }
            }
            (ParserState::InParameter { name: _, tool }, Event::End(_)) => StateUpdate {
                new_state: ParserState::InTool(tool.clone()),
                new_tool: self.current_tool.clone(),
                event: None,
            },
            (ParserState::InTool(_), Event::End(_)) => StateUpdate {
                new_state: ParserState::Initial,
                new_tool: None,
                event: Some(ToolCallEvent::ToolEnd),
            },
            _ => StateUpdate {
                new_state: state.clone(),
                new_tool: self.current_tool.clone(),
                event: None,
            },
        }
    }

    fn apply_update(&mut self, update: StateUpdate) {
        self.state = update.new_state;
        if let Some(tool) = update.new_tool {
            self.current_tool = Some(tool);
        }
    }
}

impl Stream for ToolCallStream {
    type Item = Result<ToolCallEvent, XmlError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.position >= self.buffer.len() {
            return Poll::Ready(None);
        }

        let mut reader = self.get_reader();
        let mut buf = Vec::new();

        match reader.read_event_into(&mut buf) {
            Ok(event) => {
                let current_state = self.state.clone();
                let update = self.process_event(&event, &current_state);
                let result = update.event.clone();

                let event_size = match &event {
                    Event::Text(e) => e.as_ref().len(),
                    Event::Start(e) => {
                        let name_ref = e.name();
                        let name_bytes = name_ref.as_ref();
                        name_bytes.len() + 2 // < + name + >
                    }
                    Event::End(e) => {
                        let name_ref = e.name();
                        let name_bytes = name_ref.as_ref();
                        name_bytes.len() + 3 // </ + name + >
                    }
                    Event::Eof => 0,
                    _ => 1,
                };
                self.position += event_size;

                self.apply_update(update);

                if let Some(event) = result {
                    Poll::Ready(Some(Ok(event)))
                } else {
                    self.poll_next(_cx)
                }
            }
            Err(e) => Poll::Ready(Some(Err(e.into()))),
        }
    }
}

#[tokio::main]
async fn main() {
    let xml = r#"<get_weather><location>Tokyo</location><date>2024-03-21</date></get_weather>"#;

    let stream = ToolCallStream::new(xml.as_bytes());
    let mut stream = Box::pin(stream);

    while let Some(result) = stream.next().await {
        match result {
            Ok(event) => match event {
                ToolCallEvent::ToolStart(name) => println!("ツール開始: {}", name),
                ToolCallEvent::Parameter { name, value } => {
                    println!("パラメータ: {} = {}", name, value)
                }
                ToolCallEvent::ToolEnd => println!("ツール終了"),
                ToolCallEvent::Error(err) => println!("ツールエラー: {}", err),
            },
            Err(e) => println!("エラー: {}", e),
        }
    }

    println!("---イベントストリーム終了---");
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    

    #[tokio::test]
    async fn test_stream_parser() {
        let mut stream = ToolCallStream::new(b"");

        // XMLを一文字ずつ送信
        let xml = r#"<get_weather><location>Tokyo</location></get_weather>"#;
        for c in xml.chars() {
            stream.push_data(c.to_string().as_bytes());
        }

        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }

        // イベントを検証
        assert!(
            matches!(events[0], Ok(ToolCallEvent::ToolStart(ref name)) if name == "get_weather")
        );
        assert!(
            matches!(events[1], Ok(ToolCallEvent::Parameter { ref name, ref value }) 
            if name == "location" && value == "Tokyo")
        );
        assert!(matches!(events[2], Ok(ToolCallEvent::ToolEnd)));
    }
}

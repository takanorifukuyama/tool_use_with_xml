use quick_xml::Reader;
use quick_xml::events::Event;
use serde::Deserialize;
use std::collections::HashMap;

// パースエラーを表すEnum
#[derive(thiserror::Error, Debug)]
pub enum ToolParseError {
    #[error("XML parsing error: {0}")]
    XmlError(#[from] quick_xml::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Expected start tag, found {0:?}")]
    ExpectedStartTag(String),
    #[error("Expected end tag {expected}, found {found}")]
    MismatchedEndTag { expected: String, found: String },
    #[error("Unexpected end of file")]
    UnexpectedEof,
    #[error("Tool name not found")]
    ToolNameNotFound,
    #[error("Invalid XML structure")]
    InvalidStructure,
    #[error("No tool XML found in the input text")]
    NoToolXmlFound,
}

// パースされたツール呼び出しを表す構造体
#[derive(Debug, PartialEq, Deserialize, Clone)]
pub struct ToolCall {
    pub tool_name: String,
    pub parameters: HashMap<String, String>,
}

/// LLMの応答テキストから最初のツール呼び出しXMLを抽出しパースする関数
pub fn parse_tool_call(text: &str) -> Result<ToolCall, ToolParseError> {
    // 簡易的なXMLブロック抽出（より堅牢な方法も検討可）
    // < で始まり > で終わるタグを探し、そのタグ名で囲まれたブロックを探す
    let mut tool_name = None;
    let mut xml_start_index = None;
    let mut xml_end_index = None;

    if let Some(start_tag_start) = text.find('<') {
        if let Some(start_tag_end) = text[start_tag_start..].find('>') {
            let potential_tool_name = &text[start_tag_start + 1..start_tag_start + start_tag_end];
            // 簡単のため、パラメータを持たないタグやコメントなどは無視
            if !potential_tool_name.starts_with('/')
                && !potential_tool_name.starts_with('?')
                && !potential_tool_name.starts_with('!')
                && potential_tool_name.contains(char::is_alphanumeric)
            {
                let end_tag = format!("</{}>", potential_tool_name);
                if let Some(end_tag_start) = text.find(&end_tag) {
                    tool_name = Some(potential_tool_name.to_string());
                    xml_start_index = Some(start_tag_start);
                    xml_end_index = Some(end_tag_start + end_tag.len());
                }
            }
        }
    }

    let tool_name = tool_name.ok_or(ToolParseError::NoToolXmlFound)?;
    let xml_content = &text[xml_start_index.unwrap()..xml_end_index.unwrap()];

    // quick-xml でパース
    let mut reader = Reader::from_str(xml_content);
    reader.trim_text(true); // テキスト前後の空白をトリム

    let mut params = HashMap::new();
    let mut current_param_name: Option<String> = None;

    // ルート要素の開始タグを読み飛ばす
    loop {
        match reader.read_event()? {
            Event::Start(e) if e.name().as_ref() == tool_name.as_bytes() => break,
            Event::Eof => return Err(ToolParseError::ToolNameNotFound), // 予期せぬ終了
            _ => {} // 他のイベント（コメントなど）は無視
        }
    }

    // パラメータ要素を読み取るループ
    loop {
        match reader.read_event()? {
            // パラメータの開始タグ <param_name>
            Event::Start(e) => {
                let tag_name = String::from_utf8(e.name().as_ref().to_vec())
                    .map_err(|_| ToolParseError::InvalidStructure)?; // UTF-8エラーは想定しにくいが念のため
                current_param_name = Some(tag_name);
            }
            // パラメータの値 (テキスト)
            Event::Text(e) => {
                if let Some(param_name) = &current_param_name {
                    let param_value = e.unescape()?.to_string();
                    params.insert(param_name.clone(), param_value);
                }
            }
            // パラメータの終了タグ </param_name>
            Event::End(e) => {
                if let Some(param_name) = &current_param_name {
                    let expected_tag_name = param_name.as_bytes();
                    if e.name().as_ref() != expected_tag_name {
                        return Err(ToolParseError::MismatchedEndTag {
                            expected: param_name.clone(),
                            found: String::from_utf8_lossy(e.name().as_ref()).to_string(),
                        });
                    }
                    current_param_name = None; // 現在のパラメータ処理を終了
                } else if e.name().as_ref() == tool_name.as_bytes() {
                    // ルート要素の終了タグ </tool_name> ならループ終了
                    break;
                }
            }
            // ファイル終端 (予期せぬ終了)
            Event::Eof => return Err(ToolParseError::UnexpectedEof),
            _ => {} // 他のイベント (コメント、DTDなど) は無視
        }
    }

    Ok(ToolCall {
        tool_name,
        parameters: params,
    })
}

// --- テスト ---
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn test_parse_get_weather() {
        let llm_response = r#"
明日のニューヨークの天気ですね。承知いたしました。
外部の天気予報ツールを使って最新の情報を確認しますね。

<get_weather>
  <location>New York</location>
  <date>tomorrow</date>
  <unit>fahrenheit</unit>
</get_weather>

結果が取得でき次第、すぐにお知らせします。
"#;
        let expected_params: HashMap<String, String> = [
            ("location".to_string(), "New York".to_string()),
            ("date".to_string(), "tomorrow".to_string()),
            ("unit".to_string(), "fahrenheit".to_string()),
        ]
        .iter()
        .cloned()
        .collect();

        let expected_tool_call = ToolCall {
            tool_name: "get_weather".to_string(),
            parameters: expected_params,
        };

        match parse_tool_call(llm_response) {
            Ok(tool_call) => assert_eq!(tool_call, expected_tool_call),
            Err(e) => panic!("Parse failed: {:?}", e),
        }
    }

    #[test]
    fn test_parse_write_file() {
        let llm_response = r#"
Okay, I will write the following content to the file.
<write_to_file>
<path>src/main.rs</path>
<content>
fn main() {
    println!("Hello, world!");
}
</content>
</write_to_file>
Let me know if that looks correct.
"#;
        let expected_content = r#"fn main() {
    println!("Hello, world!");
}"#;
        let expected_params: HashMap<String, String> = [
            ("path".to_string(), "src/main.rs".to_string()),
            ("content".to_string(), expected_content.to_string()),
        ]
        .iter()
        .cloned()
        .collect();

        let expected_tool_call = ToolCall {
            tool_name: "write_to_file".to_string(),
            parameters: expected_params,
        };

        match parse_tool_call(llm_response) {
            Ok(tool_call) => assert_eq!(tool_call, expected_tool_call),
            Err(e) => panic!("Parse failed: {:?}", e),
        }
    }

    #[test]
    fn test_no_tool_found() {
        let llm_response = "明日の天気は晴れでしょう。";
        match parse_tool_call(llm_response) {
            Err(ToolParseError::NoToolXmlFound) => {} // Expected error
            Ok(_) => panic!("Should have failed, but parsed successfully."),
            Err(e) => panic!("Expected NoToolXmlFound, but got {:?}", e),
        }
    }

    #[test]
    fn test_malformed_xml() {
        let llm_response = "<get_weather><location>New York</date></get_weather>"; // Mismatched tag
        match parse_tool_call(llm_response) {
            Err(_) => {} // Expected some error (likely MismatchedEndTag or XmlError)
            Ok(_) => panic!("Should have failed due to malformed XML."),
        }
    }
}

fn main() {
    let example_text = r#"
明日のニューヨークの天気ですね。承知いたしました。
外部の天気予報ツールを使って最新の情報を確認しますね。

<get_weather>
  <location>New York</location>
  <date>tomorrow</date>
  <unit>fahrenheit</unit>
</get_weather>

結果が取得でき次第、すぐにお知らせします。
"#;

    match parse_tool_call(example_text) {
        Ok(tool_call) => {
            println!("Tool name: {}", tool_call.tool_name);
            println!("Parameters:");
            for (key, value) in tool_call.parameters {
                println!("  {}: {}", key, value);
            }
        }
        Err(e) => eprintln!("Error parsing tool call: {:?}", e),
    }
}

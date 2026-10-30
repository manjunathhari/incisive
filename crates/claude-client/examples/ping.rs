//! Live smoke test: `ANTHROPIC_API_KEY=... CLAUDE_MODEL=... cargo run -p claude-client --example ping`
use claude_client::{Client, ContentBlock, Message, MessagesRequest, Role};
use secrecy::SecretString;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let key = std::env::var("ANTHROPIC_API_KEY")?;
    let model = std::env::var("CLAUDE_MODEL")?;
    let client = Client::new(SecretString::from(key));
    let req = MessagesRequest {
        model,
        max_tokens: 10,
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Reply with the single word: ready".into(),
            }],
        }],
        tools: vec![],
        tool_choice: None,
        output_config: None,
        cache_breakpoints: vec![],
    };
    let resp = client.create_message(&req).await?;
    for block in &resp.content {
        if let ContentBlock::Text { text } = block {
            println!("{text}");
        }
    }
    println!(
        "usage: input_tokens={} output_tokens={}",
        resp.usage.input_tokens, resp.usage.output_tokens
    );
    Ok(())
}

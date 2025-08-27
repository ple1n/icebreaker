use crate::{Chat, Error};

mod old;

pub fn decode(json: &str) -> Result<Chat, Error> {
    let chat: Chat = serde_json::from_str(json)?;

    Ok(chat)
}

pub fn encode(chat: &Chat) -> Result<String, Error> {

    Ok(serde_json::to_string_pretty(chat)?)
}

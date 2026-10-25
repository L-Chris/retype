use crate::{
    config::Provider,
    protocol::{Operation, Response, MAX_TEXT},
};
pub use retype_ai_core::service::*;
#[cfg(windows)]
fn key(provider: &Provider) -> Result<String, String> {
    crate::secrets::key(&provider.id).map_err(|_| "无法读取 API Key".into())
}
#[cfg(not(windows))]
fn key(_: &Provider) -> Result<String, String> {
    Ok(String::new())
}
pub fn perform(operation: Operation) -> Result<Response, String> {
    match operation {
        Operation::Models { provider } => {
            configured_provider(&provider)?;
            models(&provider).map(Response::Models)
        }
        Operation::Test { provider, model } => {
            configured_provider(&provider)?;
            if !provider.models.contains(&model) {
                return Err("测试模型未保存".into());
            }
            test_with_key(&provider, &model, &key(&provider)?)?;
            Ok(Response::Tested)
        }
        Operation::Translate { text } => {
            if text.trim().is_empty() {
                return Err("输入框为空".into());
            }
            if text.encode_utf16().count() > MAX_TEXT {
                return Err("输入框文字过长，未发送或截断原文".into());
            }
            let config = crate::config::load().map_err(|_| "无法读取翻译配置".to_string())?;
            let provider = config.selected()?;
            let text = translate(
                provider,
                &config.model,
                &text,
                &config.target,
                &config.instructions,
                &config.reasoning,
                config.timeout_seconds,
            )?;
            Ok(Response::Translation {
                text,
                preview: config.preview,
            })
        }
    }
}
fn configured_provider(provider: &Provider) -> Result<(), String> {
    let config = crate::config::load().map_err(|_| "无法读取提供商配置".to_string())?;
    if config.providers.iter().any(|saved| saved == provider) {
        Ok(())
    } else {
        Err("提供商配置已变化，请保存后重试".into())
    }
}
pub fn models(provider: &Provider) -> Result<Vec<String>, String> {
    let key = key(provider)?;
    models_with_key(provider, &key)
}
#[allow(clippy::too_many_arguments)]
pub fn translate(
    provider: &Provider,
    model: &str,
    text: &str,
    target: &str,
    instructions: &str,
    reasoning: &str,
    seconds: u64,
) -> Result<String, String> {
    let key = key(provider)?;
    translate_with_key(
        provider,
        model,
        text,
        target,
        instructions,
        reasoning,
        seconds,
        &key,
    )
}

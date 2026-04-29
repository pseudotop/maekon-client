use oneshim_api_contracts::provider_specs::ProviderRequestShape;
use serde_json::Value;

use oneshim_core::error::CoreError;
use oneshim_core::ports::ocr_provider::OcrResult;

use super::RemoteOcrProvider;

const OCR_LINE_INSTRUCTION: &str =
    "List all visible text from the image line by line. Output exactly one text item per line.";
const OCR_JSON_INSTRUCTION: &str = "Extract all visible text from the image and return strict JSON only in this schema: {\"results\":[{\"text\":\"...\",\"x\":0,\"y\":0,\"width\":0,\"height\":0,\"confidence\":0.0}]}. If exact geometry is unknown, use 0 for coordinates and size.";

#[derive(Debug, Clone, Copy)]
pub enum OcrProviderStrategy {
    Anthropic,
    OpenAi,
    Google,
}

impl TryFrom<ProviderRequestShape> for OcrProviderStrategy {
    type Error = CoreError;

    fn try_from(value: ProviderRequestShape) -> Result<Self, Self::Error> {
        match value {
            ProviderRequestShape::AnthropicMessages
            | ProviderRequestShape::AnthropicVisionMessages => Ok(Self::Anthropic),
            ProviderRequestShape::OpenAiChatCompletions
            | ProviderRequestShape::OpenAiVisionChatCompletions
            | ProviderRequestShape::OpenAiResponses => Ok(Self::OpenAi),
            ProviderRequestShape::GoogleGenerateContent
            | ProviderRequestShape::GoogleVisionAnnotate => Ok(Self::Google),
            ProviderRequestShape::BedrockConverse => Err(CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                message: "AWS Bedrock is intentionally unsupported in this build".into(),
            }),
        }
    }
}

impl OcrProviderStrategy {
    pub(crate) fn build_request_body(self, encoded: &str, media_type: &str, model: &str) -> Value {
        match self {
            Self::Google => serde_json::json!({
                "requests": [{
                    "image": { "content": encoded },
                    "features": [{
                        "type": "TEXT_DETECTION",
                        "maxResults": 64
                    }]
                }]
            }),
            Self::OpenAi => {
                let data_uri = format!("data:{media_type};base64,{encoded}");
                serde_json::json!({
                    "model": model,
                    "max_tokens": 2048,
                    "response_format": { "type": "json_object" },
                    "messages": [{
                        "role": "user",
                        "content": [
                            {
                                "type": "text",
                                "text": OCR_JSON_INSTRUCTION
                            },
                            {
                                "type": "image_url",
                                "image_url": { "url": data_uri }
                            }
                        ]
                    }]
                })
            }
            Self::Anthropic => serde_json::json!({
                "model": model,
                "max_tokens": 4096,
                "messages": [{
                    "role": "user",
                    "content": [
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": encoded
                            }
                        },
                        {
                            "type": "text",
                            "text": OCR_LINE_INSTRUCTION
                        }
                    ]
                }]
            }),
        }
    }

    pub(crate) fn parse_response(self, body: &str) -> Result<Vec<OcrResult>, CoreError> {
        match self {
            Self::Anthropic => RemoteOcrProvider::parse_claude_vision_response(body),
            Self::Google => RemoteOcrProvider::parse_google_vision_response(body),
            Self::OpenAi => RemoteOcrProvider::parse_openai_vision_response(body),
        }
    }
}

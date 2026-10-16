//! 上游平台标识。
//!
//! 由 `scripts/gen_inventory.py` 从上游 `shared/types.ts` 的 `Platform` 联合类型提取，
//! 上游新增平台时重跑脚本比对：`scripts/diff-platforms.sh`。
//!
//! 上游当前认知：72 个平台。

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    #[serde(rename = "google")]
    Google,
    #[serde(rename = "groq")]
    Groq,
    #[serde(rename = "cerebras")]
    Cerebras,
    #[serde(rename = "sail")]
    Sail,
    #[serde(rename = "aclide")]
    Aclide,
    #[serde(rename = "speka")]
    Speka,
    #[serde(rename = "auray")]
    Auray,
    #[serde(rename = "typhoon")]
    Typhoon,
    #[serde(rename = "plugsky")]
    Plugsky,
    #[serde(rename = "inferbase")]
    Inferbase,
    #[serde(rename = "simplellm")]
    Simplellm,
    #[serde(rename = "llmtr")]
    Llmtr,
    #[serde(rename = "gizmo")]
    Gizmo,
    #[serde(rename = "blockrun")]
    Blockrun,
    #[serde(rename = "moondream")]
    Moondream,
    #[serde(rename = "electronhub")]
    Electronhub,
    #[serde(rename = "experiential")]
    Experiential,
    #[serde(rename = "router9")]
    Router9,
    #[serde(rename = "septor")]
    Septor,
    #[serde(rename = "clod")]
    Clod,
    #[serde(rename = "speechify")]
    Speechify,
    #[serde(rename = "freeai")]
    Freeai,
    #[serde(rename = "blaze")]
    Blaze,
    #[serde(rename = "lucidity")]
    Lucidity,
    #[serde(rename = "airforce")]
    Airforce,
    #[serde(rename = "dreamprompting")]
    Dreamprompting,
    #[serde(rename = "waterfall")]
    Waterfall,
    #[serde(rename = "logfare")]
    Logfare,
    #[serde(rename = "bai")]
    Bai,
    #[serde(rename = "anyapi")]
    Anyapi,
    #[serde(rename = "radeon")]
    Radeon,
    #[serde(rename = "nvidia")]
    Nvidia,
    #[serde(rename = "mistral")]
    Mistral,
    #[serde(rename = "sambanova")]
    Sambanova,
    #[serde(rename = "openrouter")]
    Openrouter,
    #[serde(rename = "github")]
    Github,
    #[serde(rename = "cohere")]
    Cohere,
    #[serde(rename = "tavily")]
    Tavily,
    #[serde(rename = "exa")]
    Exa,
    #[serde(rename = "firecrawl")]
    Firecrawl,
    #[serde(rename = "linkup")]
    Linkup,
    #[serde(rename = "parallel")]
    Parallel,
    #[serde(rename = "cloudflare")]
    Cloudflare,
    #[serde(rename = "zhipu")]
    Zhipu,
    #[serde(rename = "ollama")]
    Ollama,
    #[serde(rename = "kilo")]
    Kilo,
    #[serde(rename = "pollinations")]
    Pollinations,
    #[serde(rename = "llm7")]
    Llm7,
    #[serde(rename = "huggingface")]
    Huggingface,
    #[serde(rename = "opencode")]
    Opencode,
    #[serde(rename = "ovh")]
    Ovh,
    #[serde(rename = "agnes")]
    Agnes,
    #[serde(rename = "reka")]
    Reka,
    #[serde(rename = "siliconflow")]
    Siliconflow,
    #[serde(rename = "routeway")]
    Routeway,
    #[serde(rename = "bazaarlink")]
    Bazaarlink,
    #[serde(rename = "ainative")]
    Ainative,
    #[serde(rename = "aion")]
    Aion,
    #[serde(rename = "requesty")]
    Requesty,
    #[serde(rename = "navy")]
    Navy,
    #[serde(rename = "nara")]
    Nara,
    #[serde(rename = "sealion")]
    Sealion,
    #[serde(rename = "orcarouter")]
    Orcarouter,
    #[serde(rename = "unorouter")]
    Unorouter,
    #[serde(rename = "xkiro")]
    Xkiro,
    #[serde(rename = "modelscope")]
    Modelscope,
    #[serde(rename = "qianfan")]
    Qianfan,
    #[serde(rename = "volcengine")]
    Volcengine,
    #[serde(rename = "longcat")]
    Longcat,
    #[serde(rename = "xfyun")]
    Xfyun,
    #[serde(rename = "aihorde")]
    Aihorde,
    #[serde(rename = "custom")]
    Custom,
}

impl Platform {
    /// 全部平台，顺序与上游 `Platform` 联合类型一致。
    pub const ALL: &'static [Platform] = &[
        Platform::Google,
        Platform::Groq,
        Platform::Cerebras,
        Platform::Sail,
        Platform::Aclide,
        Platform::Speka,
        Platform::Auray,
        Platform::Typhoon,
        Platform::Plugsky,
        Platform::Inferbase,
        Platform::Simplellm,
        Platform::Llmtr,
        Platform::Gizmo,
        Platform::Blockrun,
        Platform::Moondream,
        Platform::Electronhub,
        Platform::Experiential,
        Platform::Router9,
        Platform::Septor,
        Platform::Clod,
        Platform::Speechify,
        Platform::Freeai,
        Platform::Blaze,
        Platform::Lucidity,
        Platform::Airforce,
        Platform::Dreamprompting,
        Platform::Waterfall,
        Platform::Logfare,
        Platform::Bai,
        Platform::Anyapi,
        Platform::Radeon,
        Platform::Nvidia,
        Platform::Mistral,
        Platform::Sambanova,
        Platform::Openrouter,
        Platform::Github,
        Platform::Cohere,
        Platform::Tavily,
        Platform::Exa,
        Platform::Firecrawl,
        Platform::Linkup,
        Platform::Parallel,
        Platform::Cloudflare,
        Platform::Zhipu,
        Platform::Ollama,
        Platform::Kilo,
        Platform::Pollinations,
        Platform::Llm7,
        Platform::Huggingface,
        Platform::Opencode,
        Platform::Ovh,
        Platform::Agnes,
        Platform::Reka,
        Platform::Siliconflow,
        Platform::Routeway,
        Platform::Bazaarlink,
        Platform::Ainative,
        Platform::Aion,
        Platform::Requesty,
        Platform::Navy,
        Platform::Nara,
        Platform::Sealion,
        Platform::Orcarouter,
        Platform::Unorouter,
        Platform::Xkiro,
        Platform::Modelscope,
        Platform::Qianfan,
        Platform::Volcengine,
        Platform::Longcat,
        Platform::Xfyun,
        Platform::Aihorde,
        Platform::Custom,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Platform::Google => "google",
            Platform::Groq => "groq",
            Platform::Cerebras => "cerebras",
            Platform::Sail => "sail",
            Platform::Aclide => "aclide",
            Platform::Speka => "speka",
            Platform::Auray => "auray",
            Platform::Typhoon => "typhoon",
            Platform::Plugsky => "plugsky",
            Platform::Inferbase => "inferbase",
            Platform::Simplellm => "simplellm",
            Platform::Llmtr => "llmtr",
            Platform::Gizmo => "gizmo",
            Platform::Blockrun => "blockrun",
            Platform::Moondream => "moondream",
            Platform::Electronhub => "electronhub",
            Platform::Experiential => "experiential",
            Platform::Router9 => "router9",
            Platform::Septor => "septor",
            Platform::Clod => "clod",
            Platform::Speechify => "speechify",
            Platform::Freeai => "freeai",
            Platform::Blaze => "blaze",
            Platform::Lucidity => "lucidity",
            Platform::Airforce => "airforce",
            Platform::Dreamprompting => "dreamprompting",
            Platform::Waterfall => "waterfall",
            Platform::Logfare => "logfare",
            Platform::Bai => "bai",
            Platform::Anyapi => "anyapi",
            Platform::Radeon => "radeon",
            Platform::Nvidia => "nvidia",
            Platform::Mistral => "mistral",
            Platform::Sambanova => "sambanova",
            Platform::Openrouter => "openrouter",
            Platform::Github => "github",
            Platform::Cohere => "cohere",
            Platform::Tavily => "tavily",
            Platform::Exa => "exa",
            Platform::Firecrawl => "firecrawl",
            Platform::Linkup => "linkup",
            Platform::Parallel => "parallel",
            Platform::Cloudflare => "cloudflare",
            Platform::Zhipu => "zhipu",
            Platform::Ollama => "ollama",
            Platform::Kilo => "kilo",
            Platform::Pollinations => "pollinations",
            Platform::Llm7 => "llm7",
            Platform::Huggingface => "huggingface",
            Platform::Opencode => "opencode",
            Platform::Ovh => "ovh",
            Platform::Agnes => "agnes",
            Platform::Reka => "reka",
            Platform::Siliconflow => "siliconflow",
            Platform::Routeway => "routeway",
            Platform::Bazaarlink => "bazaarlink",
            Platform::Ainative => "ainative",
            Platform::Aion => "aion",
            Platform::Requesty => "requesty",
            Platform::Navy => "navy",
            Platform::Nara => "nara",
            Platform::Sealion => "sealion",
            Platform::Orcarouter => "orcarouter",
            Platform::Unorouter => "unorouter",
            Platform::Xkiro => "xkiro",
            Platform::Modelscope => "modelscope",
            Platform::Qianfan => "qianfan",
            Platform::Volcengine => "volcengine",
            Platform::Longcat => "longcat",
            Platform::Xfyun => "xfyun",
            Platform::Aihorde => "aihorde",
            Platform::Custom => "custom",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = UnknownPlatform;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "google" => Ok(Platform::Google),
            "groq" => Ok(Platform::Groq),
            "cerebras" => Ok(Platform::Cerebras),
            "sail" => Ok(Platform::Sail),
            "aclide" => Ok(Platform::Aclide),
            "speka" => Ok(Platform::Speka),
            "auray" => Ok(Platform::Auray),
            "typhoon" => Ok(Platform::Typhoon),
            "plugsky" => Ok(Platform::Plugsky),
            "inferbase" => Ok(Platform::Inferbase),
            "simplellm" => Ok(Platform::Simplellm),
            "llmtr" => Ok(Platform::Llmtr),
            "gizmo" => Ok(Platform::Gizmo),
            "blockrun" => Ok(Platform::Blockrun),
            "moondream" => Ok(Platform::Moondream),
            "electronhub" => Ok(Platform::Electronhub),
            "experiential" => Ok(Platform::Experiential),
            "router9" => Ok(Platform::Router9),
            "septor" => Ok(Platform::Septor),
            "clod" => Ok(Platform::Clod),
            "speechify" => Ok(Platform::Speechify),
            "freeai" => Ok(Platform::Freeai),
            "blaze" => Ok(Platform::Blaze),
            "lucidity" => Ok(Platform::Lucidity),
            "airforce" => Ok(Platform::Airforce),
            "dreamprompting" => Ok(Platform::Dreamprompting),
            "waterfall" => Ok(Platform::Waterfall),
            "logfare" => Ok(Platform::Logfare),
            "bai" => Ok(Platform::Bai),
            "anyapi" => Ok(Platform::Anyapi),
            "radeon" => Ok(Platform::Radeon),
            "nvidia" => Ok(Platform::Nvidia),
            "mistral" => Ok(Platform::Mistral),
            "sambanova" => Ok(Platform::Sambanova),
            "openrouter" => Ok(Platform::Openrouter),
            "github" => Ok(Platform::Github),
            "cohere" => Ok(Platform::Cohere),
            "tavily" => Ok(Platform::Tavily),
            "exa" => Ok(Platform::Exa),
            "firecrawl" => Ok(Platform::Firecrawl),
            "linkup" => Ok(Platform::Linkup),
            "parallel" => Ok(Platform::Parallel),
            "cloudflare" => Ok(Platform::Cloudflare),
            "zhipu" => Ok(Platform::Zhipu),
            "ollama" => Ok(Platform::Ollama),
            "kilo" => Ok(Platform::Kilo),
            "pollinations" => Ok(Platform::Pollinations),
            "llm7" => Ok(Platform::Llm7),
            "huggingface" => Ok(Platform::Huggingface),
            "opencode" => Ok(Platform::Opencode),
            "ovh" => Ok(Platform::Ovh),
            "agnes" => Ok(Platform::Agnes),
            "reka" => Ok(Platform::Reka),
            "siliconflow" => Ok(Platform::Siliconflow),
            "routeway" => Ok(Platform::Routeway),
            "bazaarlink" => Ok(Platform::Bazaarlink),
            "ainative" => Ok(Platform::Ainative),
            "aion" => Ok(Platform::Aion),
            "requesty" => Ok(Platform::Requesty),
            "navy" => Ok(Platform::Navy),
            "nara" => Ok(Platform::Nara),
            "sealion" => Ok(Platform::Sealion),
            "orcarouter" => Ok(Platform::Orcarouter),
            "unorouter" => Ok(Platform::Unorouter),
            "xkiro" => Ok(Platform::Xkiro),
            "modelscope" => Ok(Platform::Modelscope),
            "qianfan" => Ok(Platform::Qianfan),
            "volcengine" => Ok(Platform::Volcengine),
            "longcat" => Ok(Platform::Longcat),
            "xfyun" => Ok(Platform::Xfyun),
            "aihorde" => Ok(Platform::Aihorde),
            "custom" => Ok(Platform::Custom),
            other => Err(UnknownPlatform(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("未知上游平台: {0}")]
pub struct UnknownPlatform(pub String);

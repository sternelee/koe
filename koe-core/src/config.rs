use crate::errors::{KoeError, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Root configuration structure matching ~/.koe/config.yaml
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub asr: AsrSection,
    #[serde(default)]
    pub llm: LlmSection,
    #[serde(default)]
    pub feedback: FeedbackSection,
    #[serde(default)]
    pub dictionary: DictionarySection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AsrSection {
    #[serde(default = "default_asr_url")]
    pub url: String,
    #[serde(default)]
    pub app_key: String,
    #[serde(default)]
    pub access_key: String,
    #[serde(default = "default_resource_id")]
    pub resource_id: String,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_ms: u64,
    #[serde(default = "default_final_wait_timeout")]
    pub final_wait_timeout_ms: u64,
    #[serde(default = "default_true")]
    pub enable_ddc: bool,
    #[serde(default = "default_true")]
    pub enable_itn: bool,
    #[serde(default = "default_true")]
    pub enable_punc: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LlmSection {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub temperature: f64,
    #[serde(default = "default_top_p")]
    pub top_p: f64,
    #[serde(default = "default_llm_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default = "default_dictionary_max_candidates")]
    pub dictionary_max_candidates: usize,
    #[serde(default = "default_system_prompt_path")]
    pub system_prompt_path: String,
    #[serde(default = "default_user_prompt_path")]
    pub user_prompt_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FeedbackSection {
    #[serde(default = "default_true")]
    pub start_sound: bool,
    #[serde(default = "default_true")]
    pub stop_sound: bool,
    #[serde(default = "default_true")]
    pub error_sound: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DictionarySection {
    #[serde(default = "default_dictionary_path")]
    pub path: String,
}

// ─── Defaults ───────────────────────────────────────────────────────

fn default_asr_url() -> String {
    "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel".into()
}
fn default_resource_id() -> String {
    "volc.bigasr.sauc.duration".into()
}
fn default_connect_timeout() -> u64 {
    3000
}
fn default_final_wait_timeout() -> u64 {
    5000
}
fn default_true() -> bool {
    true
}
fn default_top_p() -> f64 {
    1.0
}
fn default_llm_timeout() -> u64 {
    8000
}
fn default_max_output_tokens() -> u32 {
    1024
}
fn default_dictionary_max_candidates() -> usize {
    200
}
fn default_dictionary_path() -> String {
    "dictionary.txt".into()
}
fn default_system_prompt_path() -> String {
    "system_prompt.txt".into()
}
fn default_user_prompt_path() -> String {
    "user_prompt.txt".into()
}

impl Default for Config {
    fn default() -> Self {
        serde_yaml::from_str("{}").unwrap()
    }
}
impl Default for AsrSection {
    fn default() -> Self {
        serde_yaml::from_str("{}").unwrap()
    }
}
impl Default for LlmSection {
    fn default() -> Self {
        serde_yaml::from_str("{}").unwrap()
    }
}
impl Default for FeedbackSection {
    fn default() -> Self {
        serde_yaml::from_str("{}").unwrap()
    }
}
impl Default for DictionarySection {
    fn default() -> Self {
        serde_yaml::from_str("{}").unwrap()
    }
}

// ─── Config Directory ───────────────────────────────────────────────

/// Returns ~/.koe/
pub fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".koe")
}

/// Returns ~/.koe/config.yaml
pub fn config_path() -> PathBuf {
    config_dir().join("config.yaml")
}

/// Resolve a path relative to config dir.
fn resolve_path(p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_dir().join(path)
    }
}

/// Resolve dictionary path (relative to config dir).
pub fn resolve_dictionary_path(config: &Config) -> PathBuf {
    resolve_path(&config.dictionary.path)
}

/// Resolve system prompt path (relative to config dir).
pub fn resolve_system_prompt_path(config: &Config) -> PathBuf {
    resolve_path(&config.llm.system_prompt_path)
}

/// Resolve user prompt path (relative to config dir).
pub fn resolve_user_prompt_path(config: &Config) -> PathBuf {
    resolve_path(&config.llm.user_prompt_path)
}

// ─── Environment Variable Substitution ──────────────────────────────

/// Replace ${VAR_NAME} patterns with environment variable values.
fn substitute_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    // Simple regex-free approach
    loop {
        let start = match result.find("${") {
            Some(pos) => pos,
            None => break,
        };
        let end = match result[start + 2..].find('}') {
            Some(pos) => start + 2 + pos,
            None => break,
        };
        let var_name = &result[start + 2..end];
        let value = std::env::var(var_name).unwrap_or_default();
        result = format!("{}{}{}", &result[..start], value, &result[end + 1..]);
    }
    result
}

// ─── Load & Ensure ─────────────────────────────────────────────────

/// Load config from ~/.koe/config.yaml.
/// Performs environment variable substitution before parsing.
pub fn load_config() -> Result<Config> {
    let path = config_path();

    if !path.exists() {
        return Err(KoeError::Config(format!(
            "config file not found: {}",
            path.display()
        )));
    }

    let raw = std::fs::read_to_string(&path)
        .map_err(|e| KoeError::Config(format!("read {}: {e}", path.display())))?;

    let substituted = substitute_env_vars(&raw);

    let config: Config = serde_yaml::from_str(&substituted)
        .map_err(|e| KoeError::Config(format!("parse {}: {e}", path.display())))?;

    Ok(config)
}

/// Ensure ~/.koe/ exists with default config.yaml and dictionary.txt.
/// Returns true if files were created (first launch).
pub fn ensure_defaults() -> Result<bool> {
    let dir = config_dir();
    let config_file = config_path();
    let dict_file = dir.join("dictionary.txt");
    let system_prompt_file = dir.join("system_prompt.txt");
    let user_prompt_file = dir.join("user_prompt.txt");

    let mut created = false;

    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| KoeError::Config(format!("create {}: {e}", dir.display())))?;
        created = true;
    }

    let defaults: &[(&std::path::Path, &str)] = &[
        (&config_file, DEFAULT_CONFIG_YAML),
        (&dict_file, DEFAULT_DICTIONARY_TXT),
        (&system_prompt_file, DEFAULT_SYSTEM_PROMPT),
        (&user_prompt_file, DEFAULT_USER_PROMPT),
    ];

    for (path, content) in defaults {
        if !path.exists() {
            std::fs::write(path, content)
                .map_err(|e| KoeError::Config(format!("write {}: {e}", path.display())))?;
            log::info!("created default: {}", path.display());
            created = true;
        }
    }

    Ok(created)
}

const DEFAULT_CONFIG_YAML: &str = r#"# Koe - Voice Input Tool Configuration
# ~/.koe/config.yaml

asr:
  # Doubao (豆包) Streaming ASR
  url: "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel"
  app_key: ""          # X-Api-App-Key (火山引擎 App ID)
  access_key: ""       # X-Api-Access-Key (火山引擎 Access Token)
  resource_id: "volc.bigasr.sauc.duration"
  connect_timeout_ms: 3000
  final_wait_timeout_ms: 5000
  enable_ddc: true     # 语义顺滑 (去除口语重复/语气词)
  enable_itn: true     # 文本规范化 (数字、日期等)
  enable_punc: true    # 自动标点

llm:
  # OpenAI-compatible endpoint for text correction
  base_url: ""         # e.g. "https://api.openai.com/v1"
  api_key: ""          # or use ${LLM_API_KEY}
  model: ""            # e.g. "gpt-4o-mini"
  temperature: 0
  top_p: 1
  timeout_ms: 8000
  max_output_tokens: 1024
  dictionary_max_candidates: 200
  system_prompt_path: "system_prompt.txt"  # relative to ~/.koe/
  user_prompt_path: "user_prompt.txt"      # relative to ~/.koe/

feedback:
  start_sound: true
  stop_sound: true
  error_sound: true

dictionary:
  path: "dictionary.txt"  # relative to ~/.koe/
"#;

const DEFAULT_DICTIONARY_TXT: &str = r#"# Koe User Dictionary
# One term per line. These terms are prioritized during LLM correction.
# Lines starting with # are comments.

"#;

const DEFAULT_SYSTEM_PROMPT: &str = "\
You are a speech-to-text post-processor for a software developer. Your task is to apply minimal corrections to ASR output that may contain a mix of Chinese and English, with frequent technical terminology.

Rules:
1. Preserve the original meaning. Do not expand, summarize, or restyle.
2. Mixed Chinese-English is intentional. Keep the speaker's language choices as-is. Do not translate Chinese to English or vice versa.
3. Capitalization: fix English words to their correct casing. This is especially important for technical terms:
   - Programming languages: Python, JavaScript, TypeScript, Rust, Go, Java, C++, Ruby, Swift, Kotlin
   - Brands/services: GitHub, GitLab, Cloudflare, AWS, GCP, Azure, Docker, Kubernetes, Redis, PostgreSQL, MySQL, MongoDB, Nginx, Node.js, Next.js, Vercel, Supabase, Firebase, Terraform, Ansible
   - Protocols/formats: HTTP, HTTPS, SSH, TCP, UDP, DNS, API, REST, GraphQL, gRPC, JSON, YAML, TOML, XML, HTML, CSS, SQL, WebSocket
   - Tools/concepts: CLI, SDK, IDE, CI/CD, DevOps, macOS, iOS, Linux, Ubuntu, npm, pip, cargo, Git, VS Code, Xcode, Vim, Neovim
   - Acronyms: URL, URI, CDN, VPN, LLM, ASR, TTS, OCR, NLP, AI, ML, GPU, CPU, RAM, SSD, IP, OAuth, JWT, CORS
   - Always capitalize the first letter of sentences.
4. Spacing: insert a half-width space between Chinese and English/numbers (e.g. \"使用Python\" -> \"使用 Python\", \"有3个\" -> \"有 3 个\"). No space between English words and Chinese punctuation.
5. Punctuation: use Chinese punctuation in Chinese context (，。！？：；) and English punctuation in English context. Do not mix. Use \"……\" instead of \"...\". Chinese sentences must end with punctuation.
6. Prefer terms, proper nouns, and spellings from the user dictionary when provided. The dictionary takes highest priority.
7. Remove filler words that carry no semantic meaning, such as 嗯, 啊, 哦, 呃, 这个, 那个, 就是, well, like, you know, um, uh, so basically.
8. Do not remove words that are clearly names, terms, titles, quoted content, or fixed expressions.
9. Code-related terms should keep their conventional form: e.g. \"main 函数\" not \"mian 函数\", \"npm install\" not \"NPM install\", \"git push\" not \"Git Push\" (subcommands stay lowercase).
10. Output only the corrected text. No explanations, no JSON, no quotation marks.";

const DEFAULT_USER_PROMPT: &str = "\
ASR transcript:
{{asr_text}}

User dictionary:
{{dictionary_entries}}

Output the corrected text only.";

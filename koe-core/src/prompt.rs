use std::path::Path;

/// Load system prompt from file, or return built-in default.
/// cbindgen:ignore
pub fn load_system_prompt(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                log::warn!("system prompt file is empty, using built-in default");
                build_default_system_prompt()
            } else {
                log::info!("loaded system prompt from {}", path.display());
                trimmed.to_string()
            }
        }
        Err(e) => {
            log::warn!(
                "failed to load system prompt from {}: {e}, using built-in default",
                path.display()
            );
            build_default_system_prompt()
        }
    }
}

/// Load user prompt template from file, or return built-in default.
/// The template should contain {{asr_text}} and {{dictionary_entries}} placeholders.
/// cbindgen:ignore
pub fn load_user_prompt_template(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                log::warn!("user prompt file is empty, using built-in default");
                build_default_user_prompt_template()
            } else {
                log::info!("loaded user prompt template from {}", path.display());
                trimmed.to_string()
            }
        }
        Err(e) => {
            log::warn!(
                "failed to load user prompt from {}: {e}, using built-in default",
                path.display()
            );
            build_default_user_prompt_template()
        }
    }
}

/// Render the user prompt by replacing placeholders in the template.
/// cbindgen:ignore
pub fn render_user_prompt(
    template: &str,
    asr_text: &str,
    dictionary_entries: &[String],
    interim_history: &[String],
) -> String {
    let dict_str = if dictionary_entries.is_empty() {
        String::from("（无）")
    } else {
        dictionary_entries.join("\n")
    };

    let interim_str = if interim_history.is_empty() {
        String::from("（无）")
    } else {
        interim_history
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{}. {}", i + 1, t))
            .collect::<Vec<_>>()
            .join("\n")
    };

    template
        .replace("{{asr_text}}", asr_text)
        .replace("{{dictionary_entries}}", &dict_str)
        .replace("{{interim_history}}", &interim_str)
}

/// Built-in default system prompt.
/// cbindgen:ignore
fn build_default_system_prompt() -> String {
    include_str!("default_system_prompt.txt").trim().to_string()
}

/// Built-in default user prompt template.
/// cbindgen:ignore
fn build_default_user_prompt_template() -> String {
    include_str!("default_user_prompt.txt").trim().to_string()
}

/// Detect a degenerate LLM rewrite driven by the dictionary in the prompt.
///
/// Small local models (e.g. a 0.6B) fixate on the dictionary section instead
/// of rewriting the ASR text, in two observed shapes:
///
/// 1. **Dump** — the model echoes the candidate list back, so the output is a
///    concatenation of dozens of dictionary entries the user never spoke,
///    burying the actual transcription.
/// 2. **Collapse** — the input contains a dictionary term (e.g. "ASR"); the
///    model latches onto that one entry and emits *only* it, discarding all
///    other spoken content.
///
/// Both erase the user's words, so we reject the output and let the caller
/// fall back to the raw ASR text. This is the output-side sibling of the
/// trim-aware empty guard, which catches blank *input*.
/// cbindgen:ignore
pub fn looks_like_dictionary_artifact(
    output: &str,
    asr_text: &str,
    dictionary_entries: &[String],
) -> bool {
    if dictionary_entries.is_empty() {
        return false;
    }

    let output_lower = output.to_lowercase();
    let asr_lower = asr_text.to_lowercase();

    // --- Dump: many dictionary terms appear in the output that the user never
    // spoke. Terms the user actually said are excluded, so a normal rewrite
    // (which injects ~none) cannot reach the threshold.
    let leaked = dictionary_entries
        .iter()
        .filter(|e| {
            let el = e.to_lowercase();
            output_lower.contains(&el) && !asr_lower.contains(&el)
        })
        .count();
    // Absolute floor (so short, legitimately term-heavy rewrites pass) AND half
    // the candidate list (so a large dictionary does not lower the bar).
    let dump_threshold = (dictionary_entries.len() / 2).max(8);
    if leaked >= dump_threshold {
        return true;
    }

    // --- Collapse: the whole output is just a single dictionary entry while
    // the ASR text carried meaningfully more content. Strip trailing
    // sentence punctuation the model may tack on, then compare exactly so we
    // don't reject a user who genuinely only said one term.
    let out_core = output_lower
        .trim()
        .trim_end_matches(['。', '.', '，', ',', '！', '!', '？', '?', ' ']);
    let out_chars = out_core.chars().count();
    let asr_chars = asr_lower.trim().chars().count();
    if out_chars > 0
        && asr_chars >= out_chars * 3
        && dictionary_entries
            .iter()
            .any(|e| e.to_lowercase() == out_core)
    {
        return true;
    }

    false
}

/// Filter dictionary candidates to reduce prompt size.
/// When `max_candidates` is 0, all entries are sent without filtering.
/// When dictionary has more than `max_candidates` entries,
/// keep only those with character overlap with the ASR text.
/// cbindgen:ignore
pub fn filter_dictionary_candidates(
    dictionary: &[String],
    asr_text: &str,
    max_candidates: usize,
) -> Vec<String> {
    if max_candidates == 0 || dictionary.len() <= max_candidates {
        return dictionary.to_vec();
    }

    let asr_lower = asr_text.to_lowercase();
    let asr_chars: std::collections::HashSet<char> = asr_lower.chars().collect();

    let mut scored: Vec<(usize, &String)> = dictionary
        .iter()
        .map(|entry| {
            let entry_lower = entry.to_lowercase();
            let overlap = entry_lower
                .chars()
                .filter(|c| asr_chars.contains(c))
                .count();
            let substring_bonus =
                if asr_lower.contains(&entry_lower) || entry_lower.contains(&asr_lower) {
                    entry.len() * 10
                } else {
                    0
                };
            (overlap + substring_bonus, entry)
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .into_iter()
        .take(max_candidates)
        .map(|(_, entry)| entry.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Vec<String> {
        [
            "cc-connect", "Anthropic", "Claudecode", "cloudflared", "Shadowrocket",
            "Karabiner", "Obsidian", "DoubaoIME", "Cloudflare", "Nextcloud", "Doubao",
            "Tailscale", "sing-box", "Docmost", "Hammerspoon", "GitHub", "Sherpa-ONNX",
            "Cursor", "Tauri", "Sonnet", "Claude", "Miniflux", "Forgejo", "OpenAI",
            "FastAPI", "Docker", "Telegram", "Gemini", "Haiku", "Codex", "Lucky",
            "Xcode", "Rustls", "Opus", "Type4Me", "OKR", "Whisper", "ASR", "PTT",
            "Vercel", "Qwen", "DeepSeek", "Hevy",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn detects_dump_regurgitating_dictionary() {
        // Real failure capture (session 831): the model echoed the whole
        // candidate list back instead of rewriting.
        let dump = "cc-connectAnthropicClaudecodecloudflaredShadowrocketKarabinerObsidianDoubaoIMECloudflareNextcloudDoubaoTailscalesing-boxDocmostHammerspoonGitHubSherpa-ONNXCursorTauriSonnetClaudeMinifluxForgejoOpenAIFastAPIDockerTelegramGeminiHaikuCodexLuckyXcodeRustlsOpusType4MeOKRWhisperASRPTTVercelQwenDeepSeekHevy";
        assert!(looks_like_dictionary_artifact(dump, "测试一下", &dict()));
    }

    #[test]
    fn detects_collapse_to_single_dictionary_term() {
        // Reported bug: a sentence containing "ASR" collapses to just "ASR".
        let asr = "当输入里头有 ASR 这三个字母的时候，输出其他的全部消失";
        assert!(looks_like_dictionary_artifact("ASR", asr, &dict()));
        // Trailing punctuation the model may append must not defeat the guard.
        assert!(looks_like_dictionary_artifact("ASR。", asr, &dict()));
    }

    #[test]
    fn passes_normal_rewrite() {
        // A genuine cleanup that keeps the user's content and a spoken term.
        let asr = "嗯那个我在用 Claude 写代码";
        assert!(!looks_like_dictionary_artifact(
            "我在用 Claude 写代码",
            asr,
            &dict()
        ));
    }

    #[test]
    fn passes_user_who_only_said_one_term() {
        // If the user genuinely just spoke "ASR", a short output is correct.
        assert!(!looks_like_dictionary_artifact("ASR", "ASR", &dict()));
    }

    #[test]
    fn passes_legit_list_of_spoken_terms() {
        // User dictates a list of products — all terms were actually spoken,
        // so none count as leaked and the output must survive.
        let asr = "我对比了 Docker、Tailscale、Cloudflare、Nextcloud、Forgejo、Miniflux";
        let out = "我对比了 Docker、Tailscale、Cloudflare、Nextcloud、Forgejo、Miniflux";
        assert!(!looks_like_dictionary_artifact(out, asr, &dict()));
    }

    #[test]
    fn empty_dictionary_never_flags() {
        assert!(!looks_like_dictionary_artifact("ASR", "a long spoken sentence here", &[]));
    }
}

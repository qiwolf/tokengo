//! 模型名的「这是不是同一个名字」判定。
//!
//! 为什么要有它：查别名、判「这条渠道认不认这个名字」都是按字符串比的，而上游与
//! 客户端写同一个模型名的写法经常不一样 —— `deepseek-v4.1-flash`、
//! `DeepSeek_V4.1_Flash`、`deepseek-v4.1-flash (siliconflow)` 在写法上差一点，
//! 指的却是同一行配置。差一点查不到就**静默退化**：别名失效（发上游的名字变回客户端的
//! 名字，多半 404）、能看图标记失效（带图请求 422）、丢字段失效 —— 界面上还看不出哪错了。
//!
//! 归一规则照 freellmapi `normalizeGroupKey` 的口径：
//! 1. 反复剥掉行尾成对的 `(...)` 括注（provider / variant）与行尾的 `free`
//!    （`xxx Free` 是价格档，不是另一个模型）；
//! 2. 空格 / 下划线 / 连字符统一当成一个分隔符（连续几个并成一个空格）；
//! 3. 大小写不敏感。
//!
//! **有意义的符号一个都不动**：`+`（`command r` 与 `command r+` 是两个模型）、
//! `.`（版本号 `3.3`）、`/` 与 `:`（`BAAI/bge-m3`、`gpt-oss:120b` 里它们是名字的一部分，
//! 拿掉就撞车了）。

/// 行尾括注 / 行尾 free 剥完、分隔符归一、转小写之后的模型名。
pub fn normalize_model_name(name: &str) -> String {
    let mut cur: String = name.trim().to_string();
    loop {
        // 分隔符先归一：这样 `-free` 与 ` free` 走同一条判定，剥完也不会留下双空格。
        cur = collapse_separators(&cur);
        let before = cur.clone();

        // 行尾成对的 `(...)`：内容里还有括号的不认（不是一对），免得把 `(a)(b` 这种切坏。
        if let Some(stripped) = strip_trailing_parens(&cur) {
            cur = stripped;
        } else if let Some(stripped) = strip_trailing_free(&cur) {
            cur = stripped;
        }

        if cur == before {
            break;
        }
    }
    cur.to_lowercase()
}

/// 两个模型名是不是同一个名字（按上面的规则归一之后比）。
pub fn same_model_name(a: &str, b: &str) -> bool {
    normalize_model_name(a) == normalize_model_name(b)
}

/// 空格 / 下划线 / 连字符 → 一个空格；首尾不留空白。
fn collapse_separators(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for ch in s.trim().chars() {
        if ch == ' ' || ch == '\t' || ch == '_' || ch == '-' {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    out
}

/// 行尾是不是一对 `(...)`（里面没有别的括号），是就把它连同前面的空白一起去掉。
fn strip_trailing_parens(s: &str) -> Option<String> {
    let t = s.trim_end();
    if !t.ends_with(')') {
        return None;
    }
    let open = t.rfind('(')?;
    let inner = &t[open + 1..t.len() - 1];
    if inner.contains('(') || inner.contains(')') {
        return None;
    }
    let head = t[..open].trim_end();
    if head == t {
        return None;
    }
    Some(head.to_string())
}

/// 行尾的 `free`（价格档）：要单独成词（前面有分隔符）才剥，
/// 免得把真叫 `free` 的模型名本身剥成空字符串。
fn strip_trailing_free(s: &str) -> Option<String> {
    let t = s.trim_end();
    if t.len() <= 4 {
        return None;
    }
    // 按字节切之前先问一句「这里是不是字符边界」：模型名里带中文时
    // `len - 4` 可能落在一个汉字中间，直接 split_at 会 panic。
    let idx = t.len() - 4;
    if !t.is_char_boundary(idx) {
        return None;
    }
    let (head, tail) = t.split_at(idx);
    if !tail.eq_ignore_ascii_case("free") {
        return None;
    }
    if !head.ends_with(' ') {
        return None;
    }
    Some(head.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_and_separator_spelling_is_the_same_name() {
        assert!(same_model_name(
            "deepseek-v4.1-flash",
            "DeepSeek_V4.1_Flash"
        ));
        assert!(same_model_name("Qwen3 Coder", "qwen3-coder"));
        assert!(same_model_name("  auto  ", "auto"));
        assert!(same_model_name("qwen3-vl-32b", "qwen3_vl 32b"));
    }

    #[test]
    fn a_trailing_parenthetical_or_free_is_not_part_of_the_name() {
        assert!(same_model_name(
            "deepseek-v4.1-flash (siliconflow)",
            "deepseek-v4.1-flash"
        ));
        assert!(same_model_name(
            "DeepSeek V4.1 Flash Free",
            "deepseek v4.1 flash"
        ));
        assert!(same_model_name("qwen3-coder (free)", "qwen3-coder"));
    }

    #[test]
    fn meaningful_symbols_are_never_merged() {
        // `+` 是型号的一部分（command r 与 command r+ 是两个模型）
        assert!(!same_model_name("command r", "command r+"));
        // `.` 是版本号
        assert!(!same_model_name("gemma-3.3", "gemma-33"));
        // `/` 与 `:` 是名字的一部分：不能把 `baai/bge-m3` 当成 `bge-m3`
        assert!(!same_model_name("BAAI/bge-m3", "bge-m3"));
        assert!(!same_model_name("gpt-oss:120b", "gpt-oss"));
    }

    #[test]
    fn a_model_literally_named_free_stays_free() {
        assert_eq!(normalize_model_name("free"), "free");
        assert_eq!(normalize_model_name("Free"), "free");
    }

    #[test]
    fn a_name_that_is_not_ascii_at_the_tail_does_not_panic() {
        // 名字尾巴是汉字时，按字节切「是不是 free」会切进字符中间 —— 这条守它。
        assert_eq!(normalize_model_name("智谱新"), "智谱新");
        assert_eq!(normalize_model_name("新模型"), "新模型");
        assert!(same_model_name("智谱新", "智谱新"));
    }

    #[test]
    fn what_normalization_actually_produces() {
        assert_eq!(
            normalize_model_name(" DeepSeek-V4.1-Flash "),
            "deepseek v4.1 flash"
        );
        assert_eq!(
            normalize_model_name("Qwen/Qwen3-VL-32B-Instruct (硅基流动) free"),
            "qwen/qwen3 vl 32b instruct"
        );
    }
}

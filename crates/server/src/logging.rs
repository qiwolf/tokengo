//! 运行日志：**面板里能看、能按级别与关键词筛、能临时开 debug，文件有上限**。
//!
//! 为什么要有这一层（用户 2026-10-18 要求「方便使用过程中出现 bug 可以排查问题」）：
//!
//! - journald 里确实有日志，但要看它得 SSH 进来、还得会用 `journalctl` —— 出问题时
//!   用户手上只有面板，所以面板得能直接看到最近的日志。
//! - 只往 journald 写等于**没有留档**：容器重启、日志轮转之后现场就没了。
//! - 但文件必须**有上限**：这台机器上吃过教训（编译缓存一天涨到 12 GB），
//!   日志更不能让它无限长 —— 单文件 4 MB、最多留 3 个，超了就轮转。
//!
//! 组成：
//!   `LogBuffer`  —— 内存里留最近 N 行（面板「日志」页读它，重启即清空）
//!   `TeeWriter`  —— 一个 `MakeWriter`：一行同时进内存缓冲与文件（文件带轮转）
//!   `LogControl` —— 运行时改日志级别（面板上点「临时开 debug」走它），可限时自动收回
//!
//! 级别默认 `info`；`RUST_LOG` 环境变量优先（部署时想长期开 debug 就设它）。

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 内存里留多少行（面板一页最多看 1000 行，留 2000 足够回溯）。
pub const BUFFER_LINES: usize = 2000;
const LINE_MAX_BYTES: usize = 8192;
const BUFFER_MAX_BYTES: usize = 2 * 1024 * 1024;
const TRUNCATED: &str = " [truncated]";

/// Keep truncation visible and include its marker in the byte budget.
fn bounded_line(text: &str, limit: usize, truncated: bool) -> String {
    if !truncated && text.len() <= limit { return text.to_owned(); }
    let marker = &TRUNCATED[..TRUNCATED.len().min(limit)];
    let mut end = text.len().min(limit.saturating_sub(marker.len()));
    while !text.is_char_boundary(end) { end -= 1; }
    format!("{}{}", &text[..end], marker)
}
/// 单个日志文件的上限。
pub const FILE_MAX_BYTES: u64 = 4 << 20;
/// 连同当前文件最多留几个（当前 + 2 个历史）。
pub const FILE_KEEP: usize = 3;
/// 默认级别（没有 RUST_LOG 时）。
pub const DEFAULT_FILTER: &str = "tokengo_server=info,tower_http=warn";

/// 内存里的最近日志。**只读快照给面板**，写入走 `push`。
#[derive(Default)]
struct BufferedLines {
    lines: VecDeque<String>,
    bytes: usize,
}

pub struct LogBuffer {
    lines: Mutex<BufferedLines>,
    cap: usize,
}

impl LogBuffer {
    pub fn new(cap: usize) -> Arc<Self> {
        Arc::new(Self {
            lines: Mutex::new(BufferedLines { lines: VecDeque::with_capacity(cap.min(1024)), bytes: 0 }),
            cap: cap.max(1),
        })
    }

    pub fn push(&self, line: String) {
        let line = bounded_line(&line, LINE_MAX_BYTES, false);
        let mut q = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        while q.lines.len() >= self.cap || q.bytes + line.len() > BUFFER_MAX_BYTES {
            if let Some(old) = q.lines.pop_front() { q.bytes -= old.len(); } else { break; }
        }
        q.bytes += line.len();
        q.lines.push_back(line);
    }

    pub fn len(&self) -> usize {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 面板取日志：先按级别门槛、再按关键词过滤，**新的在前**，最多 `limit` 行。
    pub fn query(&self, level: Option<&str>, keyword: Option<&str>, limit: usize) -> Vec<String> {
        let floor = severity(level.unwrap_or(""));
        let kw = keyword.unwrap_or("").trim().to_lowercase();
        let q = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        q.lines.iter()
            .rev()
            .filter(|l| severity_of(l) >= floor)
            .filter(|l| kw.is_empty() || l.to_lowercase().contains(&kw))
            .take(limit.max(1))
            .cloned()
            .collect()
    }
}

/// 级别名 → 严重度（数字越大越严重）。空字符串 = 不过滤（0）。
fn severity(name: &str) -> u8 {
    match name.trim().to_ascii_lowercase().as_str() {
        "error" => 3,
        "warn" | "warning" => 2,
        "info" => 1,
        _ => 0,
    }
}

/// 一行日志自己的严重度（读的是 `tracing` 打出来的级别词）。
fn severity_of(line: &str) -> u8 {
    // 形如 `2026-10-18T08:24:18.936761Z ERROR tokengo_server::gateway: ...`
    if line.contains(" ERROR") || line.starts_with("ERROR") {
        3
    } else if line.contains(" WARN") || line.starts_with("WARN") {
        2
    } else {
        1
    }
}

/// 文件写入状态（带轮转）。
struct FileState {
    path: PathBuf,
    file: Option<File>,
    written: u64,
    max_bytes: u64,
    keep: usize,
}

impl FileState {
    fn open(dir: &Path) -> Option<Self> {
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join("tokengo.log");
        let mut st = Self {
            path,
            file: None,
            written: 0,
            max_bytes: FILE_MAX_BYTES,
            keep: FILE_KEEP,
        };
        st.written = std::fs::metadata(&st.path).map(|m| m.len()).unwrap_or(0);
        st.reopen();
        Some(st)
    }

    fn reopen(&mut self) {
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok();
        self.written = self.file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
    }

    /// 写一段；超上限就轮转（把当前文件挪成 `.1`，老的往后顺延，超出 `keep` 的删掉）。
    fn write(&mut self, bytes: &[u8]) {
        if self.file.is_none() {
            return;
        }
        let text = String::from_utf8_lossy(bytes);
        let text = bounded_line(&text, self.max_bytes as usize, false);
        let bytes = text.as_bytes();
        if self.written + bytes.len() as u64 > self.max_bytes {
            self.rotate();
        }
        // A failed rename can leave the original full file in place. Do not
        // append beyond its budget merely because rotation was attempted.
        if self.written + bytes.len() as u64 > self.max_bytes { return; }
        if let Some(f) = self.file.as_mut() {
            if f.write_all(bytes).is_ok() {
                self.written += bytes.len() as u64;
            } else {
                // 写失败（磁盘满/被删）就别再重试刷屏了，下一轮轮转会重开
                self.file = None;
            }
        }
    }

    fn rotate(&mut self) {
        self.file = None;
        let base = self.path.clone();
        // 留多少个文件就只留多少个：当前 + .1 … .{keep-1}。
        // 先删掉最老的那一格，再把老的往后挪，最后当前 → .1。
        // （这里第一版算错过：写成「依次后移」会多出一格，实测留下了 4 个文件。）
        if self.keep > 1 {
            let _ = std::fs::remove_file(base.with_extension(format!("log.{}", self.keep - 1)));
            for i in (1..self.keep - 1).rev() {
                let from = base.with_extension(format!("log.{i}"));
                let to = base.with_extension(format!("log.{}", i + 1));
                if from.exists() {
                    let _ = std::fs::rename(&from, &to);
                }
            }
        }
        let first = base.with_extension("log.1");
        let _ = std::fs::rename(&base, &first);
        self.written = 0;
        self.reopen();
        // Do not inject an unaccounted rotation notice into the new file.
        // Every byte in a log file is written and counted by write().
    }
}

/// 一个写入端：一行同时进内存缓冲和文件。给 `fmt` 层当 `MakeWriter` 用。
pub struct TeeWriter {
    buf: Arc<LogBuffer>,
    file: Arc<Mutex<Option<FileState>>>,
    file_path: Arc<Mutex<Option<PathBuf>>>,
    pending: Vec<u8>,
    truncated: bool,
}

impl Clone for TeeWriter {
    fn clone(&self) -> Self {
        Self { buf: self.buf.clone(), file: self.file.clone(), file_path: self.file_path.clone(),
            pending: Vec::new(), truncated: false }
    }
}

impl Drop for TeeWriter {
    fn drop(&mut self) { self.finish_line(); }
}

impl TeeWriter {
    pub fn new(buf: Arc<LogBuffer>, dir: Option<PathBuf>) -> Self {
        let (file, path) = match dir.as_deref().map(FileState::open) {
            Some(Some(st)) => {
                let p = st.path.clone();
                (Some(st), Some(p))
            }
            _ => (None, None),
        };
        Self {
            buf,
            file: Arc::new(Mutex::new(file)),
            file_path: Arc::new(Mutex::new(path)),
            pending: Vec::new(),
            truncated: false,
        }
    }

    /// 日志文件的绝对路径（面板上显示给用户，方便他直接去看/下载）。
    pub fn file_path(&self) -> Option<PathBuf> {
        self.file_path.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn sink(&self, s: &str) {
        {
            let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(st) = f.as_mut() {
                st.write(s.as_bytes());
            }
        }
        // 只在完整行时推进缓冲：避免自动刷新时读到半行
        if s.ends_with('\n') {
            let line = s.trim_end_matches(['\n', '\r']).to_string();
            if !line.is_empty() {
                self.buf.push(line);
            }
        }
    }
}

impl TeeWriter {
    fn finish_line(&mut self) {
        if self.pending.is_empty() && !self.truncated { return; }
        let mut bytes = std::mem::take(&mut self.pending);
        if self.truncated {
            // A bounded prefix can end midway through a valid UTF-8 character.
            if let Err(error) = std::str::from_utf8(&bytes) {
                if error.error_len().is_none() { bytes.truncate(error.valid_up_to()); }
            }
        }
        let text = String::from_utf8_lossy(&bytes);
        let text = bounded_line(text.trim_end_matches('\r'), LINE_MAX_BYTES, self.truncated);
        self.truncated = false;
        if !text.is_empty() { self.sink(&(text + "\n")); }
    }
}

impl Write for TeeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // Bytes remain undecoded until the complete event, so a UTF-8 code point
        // split across Write calls is preserved. Each clone owns its event state.
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            let newline = part.last() == Some(&b'\n');
            let content = if newline { &part[..part.len() - 1] } else { part };
            let take = content.len().min(LINE_MAX_BYTES.saturating_sub(self.pending.len()));
            self.pending.extend_from_slice(&content[..take]);
            self.truncated |= take < content.len();
            if newline { self.finish_line(); }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.finish_line();
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TeeWriter {
    type Writer = TeeWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// 运行时日志级别控制。没有订阅者时（测试里）是空操作。
struct LogSettings {
    level: String,
    revert_at: Option<i64>,
    generation: u64,
}

pub struct LogControl {
    settings: Mutex<LogSettings>,
    setter: Mutex<Option<Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>>>,
}

impl LogControl {
    /// 没接订阅者（测试、纯库场景）：改级别只记账，不影响任何东西。
    pub fn detached() -> Arc<Self> {
        Arc::new(Self {
            settings: Mutex::new(LogSettings { level: DEFAULT_FILTER.to_string(), revert_at: None, generation: 0 }),
            setter: Mutex::new(None),
        })
    }

    pub fn level(&self) -> String {
        self.settings.lock().unwrap_or_else(|e| e.into_inner()).level.clone()
    }

    pub fn revert_at_ms(&self) -> Option<i64> {
        self.settings.lock().unwrap_or_else(|e| e.into_inner()).revert_at
    }

    /// Commit filter, deadline and timer identity together. Failed changes leave
    /// the active level and its existing expiry task untouched.
    pub fn configure(&self, level: &str, deadline: Option<i64>) -> Result<u64, String> {
        let spec = normalize_filter(level)?;
        let mut settings = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(setter) = self.setter.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            setter(&spec)?;
        }
        settings.generation = settings.generation.wrapping_add(1);
        settings.level = spec;
        settings.revert_at = deadline;
        Ok(settings.generation)
    }

    /// Called only after the timer's Tokio sleep completes. The generation check
    /// prevents both obsolete timers and equal-wall-clock-deadline collisions.
    pub fn expire(&self, generation: u64) -> Result<bool, String> {
        let mut settings = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        if settings.generation != generation || settings.revert_at.is_none() { return Ok(false); }
        if let Some(setter) = self.setter.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            setter(DEFAULT_FILTER)?;
        }
        settings.generation = settings.generation.wrapping_add(1);
        settings.level = DEFAULT_FILTER.to_owned();
        settings.revert_at = None;
        Ok(true)
    }

    /// Direct changes also invalidate any older expiry task.
    pub fn set_level(&self, level: &str) -> Result<(), String> {
        self.configure(level, None).map(|_| ())
    }

    pub fn clear_revert(&self) { self.set_revert_at(None); }

    pub fn set_revert_at(&self, at: Option<i64>) {
        let mut settings = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        settings.generation = settings.generation.wrapping_add(1);
        settings.revert_at = at;
    }

    fn attach(&self, f: Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>) {
        *self.setter.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }
}

/// 面板上传进来的级别词 → `tracing` 过滤串。只认这几个词，别的直接拒（别让人塞进半截表达式）。
pub fn normalize_filter(level: &str) -> Result<String, String> {
    let l = level.trim().to_ascii_lowercase();
    if l.contains('=') || l.contains(',') {
        // 已经是完整过滤串
        return Ok(level.trim().to_string());
    }
    match l.as_str() {
        "debug" => Ok("tokengo_server=debug,tower_http=info".into()),
        "info" => Ok(DEFAULT_FILTER.into()),
        "warn" => Ok("tokengo_server=warn,tower_http=warn".into()),
        "error" => Ok("tokengo_server=error,tower_http=error".into()),
        other => Err(format!("不认识的级别「{other}」：只能填 debug / info / warn / error")),
    }
}

/// 起订阅者：stdout（journald）+ 内存缓冲 + 文件（带轮转），返回（缓冲、级别控制）。
///
/// 返回 `Result`：订阅者只能装一次（测试里多次调用会失败），调用方可以忽略错误继续跑。
pub fn init(log_dir: Option<PathBuf>) -> (Arc<LogBuffer>, Arc<LogControl>, Option<PathBuf>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let buf = LogBuffer::new(BUFFER_LINES);
    let writer = TeeWriter::new(buf.clone(), log_dir);
    let file_path = writer.file_path();
    let control = LogControl::detached();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_FILTER));
    let (filter, handle) = tracing_subscriber::reload::Layer::new(filter);
    // 把「改级别」绑到 reload 句柄上：面板改级别走的就是它
    let ctl2 = control.clone();
    ctl2.attach(Arc::new(move |spec: &str| {
        handle
            .modify(|f| *f = tracing_subscriber::EnvFilter::new(spec))
            .map_err(|e| format!("换日志级别失败：{e}"))
    }));

    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(io::stdout)
                .with_target(true),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_target(true),
        )
        .try_init();
    if installed.is_err() {
        tracing::warn!("日志订阅者已经装过了（测试里常见），这一层只保留内存缓冲");
    }
    (buf, control, file_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(buf: &LogBuffer, n: usize) {
        for i in 0..n {
            buf.push(format!(
                "2026-10-18T00:00:{:02}Z INFO tokengo_server::x: 第 {i} 行",
                i % 60
            ));
        }
    }

    /// 内存缓冲满了之后丢最老的（不然长时间跑会吃内存）。
    ///
    /// 反证：把 `push` 里的 `while q.len() >= self.cap { pop_front }` 去掉 → 这条变红。
    #[test]
    fn the_buffer_keeps_only_the_newest_lines() {
        let buf = LogBuffer::new(10);
        feed(&buf, 25);
        assert_eq!(buf.len(), 10, "最多留 cap 行");
        let got = buf.query(None, None, 100);
        assert!(got[0].contains("第 24 行"), "最新的在最前：{}", got[0]);
        assert_eq!(got.len(), 10);
        assert!(
            !got.iter().any(|l| l.contains("第 14 行")),
            "太老的必须已经被挤出去"
        );
    }

    /// 级别过滤：选 warn 就只剩 WARN/ERROR；关键词大小写不敏感。
    #[test]
    fn level_and_keyword_filters_work() {
        let buf = LogBuffer::new(50);
        buf.push("2026-10-18T00:00:00Z INFO tokengo_server::a: 正常一行".into());
        buf.push("2026-10-18T00:00:01Z WARN tokengo_server::a: 渠道慢".into());
        buf.push("2026-10-18T00:00:02Z ERROR tokengo_server::a: 上游 500 请求 12345".into());

        let only_warn = buf.query(Some("warn"), None, 50);
        assert_eq!(only_warn.len(), 2, "warn 门槛要含 warn 与 error");
        assert!(!only_warn.iter().any(|l| l.contains("正常一行")));

        let only_err = buf.query(Some("error"), None, 50);
        assert_eq!(only_err.len(), 1);
        assert!(only_err[0].contains("上游 500"));

        // 按请求号找现场（大小写不敏感）
        let by_id = buf.query(None, Some("12345"), 50);
        assert_eq!(by_id.len(), 1);
        assert!(by_id[0].contains("上游 500"));

        // 全都要：不过滤
        assert_eq!(buf.query(None, None, 50).len(), 3);
    }

    /// 文件写入超上限要轮转，且**最多留 keep 个**（这台机器吃过「不设上限」的亏）。
    #[test]
    fn the_log_file_rotates_and_keeps_a_bounded_number_of_files() {
        let dir = std::env::temp_dir().join(format!("tokengo-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let buf = LogBuffer::new(50);
        let w = TeeWriter::new(buf.clone(), Some(dir.clone()));
        {
            let mut f = w.file.lock().unwrap();
            let st = f.as_mut().unwrap();
            st.max_bytes = 512; // 缩小上限好测
            st.keep = 3;
        }
        // 写 40 行、每行 100 字节 → 必然轮转多次
        for i in 0..40 {
            let mut w2 = w.clone();
            let _ = writeln!(w2, "第 {i} 行：{}", "x".repeat(80));
        }
        let files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        assert!(files.len() <= 3, "最多留 3 个文件，实际 {files:?}");
        assert!(
            files.iter().any(|f| f == "tokengo.log"),
            "当前文件必须在：{files:?}"
        );
        for f in &files {
            let n = std::fs::metadata(dir.join(f)).unwrap().len();
            assert!(n <= 1200, "{f} 竟然有 {n} 字节，轮转没生效");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 级别词只认四个；完整过滤串原样放行；别的直接拒（别让人塞乱七八糟的表达式）。
    #[test]
    fn level_words_are_normalised_and_junk_is_rejected() {
        assert!(normalize_filter("DEBUG").unwrap().contains("tokengo_server=debug"));
        assert_eq!(normalize_filter("info").unwrap(), DEFAULT_FILTER);
        assert!(normalize_filter("warn").unwrap().contains("=warn"));
        assert!(normalize_filter("tokengo_server=trace,foo=debug")
            .unwrap()
            .contains("trace"));
        assert!(normalize_filter("乱填").is_err());
    }

    /// 没接订阅者时改级别也不能崩（测试/库场景）。
    #[test]
    fn detached_control_records_the_level_without_a_subscriber() {
        let c = LogControl::detached();
        c.set_level("debug").unwrap();
        assert!(c.level().contains("debug"));
        assert!(c.set_level("乱填").is_err(), "非法级别要报错");
        assert!(c.level().contains("debug"), "报错之后保持原样");
    }
}

#[cfg(test)]
mod bounded_event_regressions {
    use super::*;

    #[test]
    fn memory_lines_are_bounded_in_utf8_and_mark_truncation() {
        let buf = LogBuffer::new(10);
        buf.push(format!("WARN {}", "诊断".repeat(10_000)));
        let lines = buf.query(None, None, 10);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].len() <= 8192, "an individual memory line must fit 8 KiB");
        assert!(lines[0].contains("[truncated]"), "lost text must be visible");
        assert!(!lines[0].contains('\u{fffd}'), "valid UTF-8 must stay valid");
    }

    #[test]
    fn memory_budget_evicts_old_lines_even_below_line_capacity() {
        let buf = LogBuffer::new(2000);
        for i in 0..600 {
            buf.push(format!("INFO event={i:04} {}", "x".repeat(8000)));
        }
        let lines = buf.query(None, None, 2000);
        assert!(lines.iter().map(String::len).sum::<usize>() <= 2 * 1024 * 1024,
            "memory retention must obey its byte budget as well as its line budget");
        assert!(lines[0].contains("event=0599"));
        assert!(!lines.iter().any(|line| line.contains("event=0000")));
    }

    #[test]
    fn every_rotated_file_obeys_the_exact_budget_including_notices() {
        let dir = std::env::temp_dir().join(format!("tokengo-strict-log-{}", uuid::Uuid::new_v4()));
        let mut state = FileState::open(&dir).unwrap();
        state.max_bytes = 512;
        for _ in 0..8 {
            state.write(&[b'x'; 480]);
            let bytes = std::fs::metadata(&state.path).unwrap().len();
            assert_eq!(state.written, bytes, "rotation notices must count toward written bytes");
            for entry in std::fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                assert!(entry.metadata().unwrap().len() <= 512, "every file must fit exactly");
            }
        }
        state.write(&[b'y'; 4096]);
        assert!(std::fs::metadata(&state.path).unwrap().len() <= 512,
            "an oversized write cannot bypass rotation bounds");
        assert!(std::fs::read_dir(&dir).unwrap().count() <= 3);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fragmented_utf8_and_multiple_lines_are_complete_events() {
        let buf = LogBuffer::new(10);
        let mut writer = TeeWriter::new(buf.clone(), None);
        let bytes = "WARN 请求=42 正常\nINFO next\n".as_bytes();
        for byte in bytes {
            writer.write_all(&[*byte]).unwrap();
        }
        assert_eq!(buf.query(None, None, 10), vec!["INFO next", "WARN 请求=42 正常"]);
    }

    #[test]
    fn one_write_with_multiple_events_keeps_separate_filterable_lines() {
        let buf = LogBuffer::new(10);
        let mut writer = TeeWriter::new(buf.clone(), None);
        writer.write_all(b"INFO good\nWARN bad\n").unwrap();
        assert_eq!(buf.query(None, None, 10), vec!["WARN bad", "INFO good"]);
        assert_eq!(buf.query(Some("warn"), None, 10), vec!["WARN bad"]);
    }

    #[test]
    fn writer_marks_oversized_events_then_recovers_for_the_next_event() {
        let buf = LogBuffer::new(10);
        let mut writer = TeeWriter::new(buf.clone(), None);
        writer.write_all(format!("WARN {}", "中文".repeat(10_000)).as_bytes()).unwrap();
        assert!(buf.is_empty(), "an unfinished event is not a completed log line");
        writer.write_all(b"\nINFO after\n").unwrap();
        let lines = buf.query(None, None, 10);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "INFO after");
        assert!(lines[1].len() <= 8192);
        assert!(lines[1].contains("[truncated]"));
        assert!(!lines[1].contains('\u{fffd}'));
    }

    #[test]
    fn flush_and_drop_preserve_unterminated_events_exactly_once() {
        let buf = LogBuffer::new(10);
        {
            let mut writer = TeeWriter::new(buf.clone(), None);
            writer.write_all("WARN 完整但无换行".as_bytes()).unwrap();
            writer.flush().unwrap();
            assert_eq!(buf.query(None, None, 10), vec!["WARN 完整但无换行"]);
            writer.flush().unwrap();
            writer.write_all(b"INFO drop-only").unwrap();
        }
        assert_eq!(buf.query(None, None, 10), vec!["INFO drop-only", "WARN 完整但无换行"]);
    }

    #[test]
    fn cloning_an_active_writer_does_not_duplicate_its_partial_event() {
        let buf = LogBuffer::new(10);
        let mut a = TeeWriter::new(buf.clone(), None);
        a.write_all(b"WARN original ").unwrap();
        let mut b = a.clone();
        b.write_all(b"INFO independent\n").unwrap();
        a.write_all(b"complete\n").unwrap();
        assert_eq!(buf.query(None, None, 10), vec!["WARN original complete", "INFO independent"]);
    }

    #[test]
    fn flushing_an_unterminated_event_preserves_it_in_the_file_too() {
        let dir = std::env::temp_dir().join(format!("tokengo-flush-log-{}", uuid::Uuid::new_v4()));
        let buf = LogBuffer::new(10);
        {
            let mut writer = TeeWriter::new(buf.clone(), Some(dir.clone()));
            writer.write_all("INFO 文件完整".as_bytes()).unwrap();
            writer.flush().unwrap();
        }
        assert_eq!(std::fs::read_to_string(dir.join("tokengo.log")).unwrap(), "INFO 文件完整\n");
        assert_eq!(buf.query(None, None, 10), vec!["INFO 文件完整"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn independent_writer_clones_do_not_mix_pending_events() {
        let buf = LogBuffer::new(10);
        let root = TeeWriter::new(buf.clone(), None);
        let mut a = root.clone();
        let mut b = root.clone();
        a.write_all(b"WARN a ").unwrap();
        b.write_all(b"INFO b\n").unwrap();
        a.write_all(b"done\n").unwrap();
        assert_eq!(buf.query(None, None, 10), vec!["WARN a done", "INFO b"]);
    }
}

#[cfg(test)]
mod expiry_generation_regressions {
    use super::*;

    #[test]
    fn equal_deadlines_still_have_distinct_timer_generations() {
        let ctl = LogControl::detached();
        let old = ctl.configure("debug", Some(42)).unwrap();
        let current = ctl.configure("debug", Some(42)).unwrap();
        assert_ne!(old, current);
        assert!(!ctl.expire(old).unwrap());
        assert!(ctl.level().contains("=debug"));
        assert!(ctl.expire(current).unwrap());
        assert_eq!(ctl.level(), DEFAULT_FILTER);
        assert!(!ctl.expire(current).unwrap(), "an expiry is applied once");
    }

    #[test]
    fn failed_filter_changes_preserve_the_valid_expiry() {
        let ctl = LogControl::detached();
        let current = ctl.configure("debug", Some(42)).unwrap();
        assert!(ctl.configure("invalid", None).is_err());
        assert_eq!(ctl.revert_at_ms(), Some(42));
        assert!(ctl.expire(current).unwrap());
    }

    #[test]
    fn filter_reload_failure_preserves_the_reported_level_and_deadline() {
        let ctl = LogControl::detached();
        let current = ctl.configure("debug", Some(42)).unwrap();
        ctl.attach(Arc::new(|_| Err("reload failed".into())));
        assert!(ctl.configure("warn", None).is_err());
        assert!(ctl.level().contains("=debug"));
        assert_eq!(ctl.revert_at_ms(), Some(42));
        assert!(ctl.expire(current).is_err());
        assert!(ctl.level().contains("=debug"));
        assert_eq!(ctl.revert_at_ms(), Some(42));
    }

    #[test]
    fn direct_level_or_deadline_mutations_invalidate_old_timers() {
        let ctl = LogControl::detached();
        let old = ctl.configure("debug", Some(42)).unwrap();
        ctl.set_level("warn").unwrap();
        assert!(!ctl.expire(old).unwrap());
        let old = ctl.configure("debug", Some(42)).unwrap();
        ctl.set_revert_at(Some(43));
        assert!(!ctl.expire(old).unwrap());
        let old = ctl.configure("debug", Some(42)).unwrap();
        ctl.clear_revert();
        assert!(!ctl.expire(old).unwrap());
    }
}

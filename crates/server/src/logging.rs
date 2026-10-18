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
/// 单个日志文件的上限。
pub const FILE_MAX_BYTES: u64 = 4 << 20;
/// 连同当前文件最多留几个（当前 + 2 个历史）。
pub const FILE_KEEP: usize = 3;
/// 默认级别（没有 RUST_LOG 时）。
pub const DEFAULT_FILTER: &str = "tokengo_server=info,tower_http=warn";

/// 内存里的最近日志。**只读快照给面板**，写入走 `push`。
pub struct LogBuffer {
    lines: Mutex<VecDeque<String>>,
    cap: usize,
}

impl LogBuffer {
    pub fn new(cap: usize) -> Arc<Self> {
        Arc::new(Self {
            lines: Mutex::new(VecDeque::with_capacity(cap.min(1024))),
            cap,
        })
    }

    pub fn push(&self, line: String) {
        let mut q = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        while q.len() >= self.cap {
            q.pop_front();
        }
        q.push_back(line);
    }

    pub fn len(&self) -> usize {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 面板取日志：先按级别门槛、再按关键词过滤，**新的在前**，最多 `limit` 行。
    pub fn query(&self, level: Option<&str>, keyword: Option<&str>, limit: usize) -> Vec<String> {
        let floor = severity(level.unwrap_or(""));
        let kw = keyword.unwrap_or("").trim().to_lowercase();
        let q = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        q.iter()
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
    if line.contains(" ERROR") {
        3
    } else if line.contains(" WARN") {
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
    }

    /// 写一段；超上限就轮转（把当前文件挪成 `.1`，老的往后顺延，超出 `keep` 的删掉）。
    fn write(&mut self, bytes: &[u8]) {
        if self.file.is_none() {
            return;
        }
        if self.written + bytes.len() as u64 > self.max_bytes {
            self.rotate();
        }
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
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "（日志超过 {} MB，已轮转到 {})", self.max_bytes >> 20, first.display());
        }
    }
}

/// 一个写入端：一行同时进内存缓冲和文件。给 `fmt` 层当 `MakeWriter` 用。
#[derive(Clone)]
pub struct TeeWriter {
    buf: Arc<LogBuffer>,
    file: Arc<Mutex<Option<FileState>>>,
    file_path: Arc<Mutex<Option<PathBuf>>>,
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

impl Write for TeeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(bytes);
        self.sink(&text);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
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
pub struct LogControl {
    level: Mutex<String>,
    /// 限时 debug 的收回到期时间（毫秒时间戳）；None = 没有限时任务
    revert_at: Mutex<Option<i64>>,
    setter: Mutex<Option<Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>>>,
}

impl LogControl {
    /// 没接订阅者（测试、纯库场景）：改级别只记账，不影响任何东西。
    pub fn detached() -> Arc<Self> {
        Arc::new(Self {
            level: Mutex::new(DEFAULT_FILTER.to_string()),
            revert_at: Mutex::new(None),
            setter: Mutex::new(None),
        })
    }

    pub fn level(&self) -> String {
        self.level.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn revert_at_ms(&self) -> Option<i64> {
        *self.revert_at.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 把级别设成 `tokengo_server=<level>,tower_http=warn`（也接受 `RUST_LOG` 那种整串）。
    pub fn set_level(&self, level: &str) -> Result<(), String> {
        let spec = normalize_filter(level)?;
        *self.level.lock().unwrap_or_else(|e| e.into_inner()) = spec.clone();
        let setter = self.setter.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match setter {
            Some(f) => f(&spec),
            None => Ok(()),
        }
    }

    pub fn clear_revert(&self) {
        *self.revert_at.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn set_revert_at(&self, at: Option<i64>) {
        *self.revert_at.lock().unwrap_or_else(|e| e.into_inner()) = at;
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

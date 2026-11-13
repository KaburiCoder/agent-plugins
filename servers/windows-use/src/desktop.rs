//! The desktop worker: one thread that owns the UI Automation client and the element
//! references handed out to the model, and runs every tool call in order.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use image::{DynamicImage, RgbaImage, imageops};
use uiautomation::UIElement;
use uiautomation::types::{TreeScope, UIProperty};
use uiautomation::variants::Variant;
use windows::Win32::UI::WindowsAndMessaging::{SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE};

use crate::geom::ScreenRect;
use crate::input::{self, ClickKind, NotPressed};
use crate::params::*;
use crate::uia::{self, Uia};
use crate::{capture, matcher, win32};

pub enum Reply {
    Text(String),
    Image { text: String, png: Vec<u8> },
}

type Job = Box<dyn FnOnce(&mut State) + Send>;

#[derive(Clone)]
pub struct Desktop {
    tx: mpsc::Sender<Job>,
}

impl Desktop {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("desktop".into())
            .spawn(move || {
                let mut state = State::new();
                for job in rx {
                    // A panic only loses that call's answer (the caller reports it).
                    let _ = catch_unwind(AssertUnwindSafe(|| job(&mut state)));
                }
            })
            .expect("desktop thread");
        Self { tx }
    }

    pub async fn run<T: Send + 'static>(&self, f: impl FnOnce(&mut State) -> T + Send + 'static) -> Result<T> {
        let (otx, orx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Box::new(move |s| {
                let _ = otx.send(f(s));
            }))
            .map_err(|_| anyhow!("작업 스레드가 멈춤"))?;
        orx.await.map_err(|_| anyhow!("작업 중 내부 오류(패닉)"))
    }
}

pub enum Target {
    Uia(UIElement),
    Hwnd(isize),
}

pub struct State {
    uia: std::result::Result<Uia, String>,
    refs: HashMap<u32, Target>,
    next_ref: u32,
}

const MAX_REFS: usize = 50_000;
const POLL: Duration = Duration::from_millis(250);

fn clip(s: &str, max: usize) -> String {
    let one_line: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    let t = one_line.trim();
    if t.chars().count() > max { format!("{}…", t.chars().take(max).collect::<String>()) } else { t.to_string() }
}

/// `\r\n` and a lone `\r` (what RichEdit keeps) as `\n`.
fn unify_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

fn same_process(actual: &str, want: &str) -> bool {
    let strip = |s: &str| s.trim().trim_end_matches(".exe").trim_end_matches(".EXE").to_lowercase();
    strip(actual) == strip(want)
}

/// Waits until `f` gives something, up to `wait_ms` (one try when 0).
fn poll<T>(wait_ms: u64, mut f: impl FnMut() -> Result<Option<T>>) -> Result<Option<T>> {
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    loop {
        if let Some(v) = f()? {
            return Ok(Some(v));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        sleep(POLL);
    }
}

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(img.clone()).to_rgb8().write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

impl State {
    fn new() -> Self {
        Self { uia: Uia::new().map_err(|e| format!("{e:#}")), refs: HashMap::new(), next_ref: 0 }
    }

    fn uia(&self) -> Result<&Uia> {
        self.uia.as_ref().map_err(|e| anyhow!("UI Automation을 쓸 수 없음: {e}"))
    }

    fn add_ref(&mut self, t: Target) -> String {
        if self.refs.len() >= MAX_REFS {
            self.refs.clear();
        }
        self.next_ref += 1;
        self.refs.insert(self.next_ref, t);
        format!("e{}", self.next_ref)
    }

    fn target(&self, r: &str) -> Result<&Target> {
        let n: u32 = r.trim().trim_start_matches('e').parse().map_err(|_| anyhow!("ref 형식이 아님: {r} (예: e12)"))?;
        self.refs.get(&n).ok_or_else(|| anyhow!("ref {r}를 모름: 오래되었으니 ui_tree나 ui_find로 다시 찾으세요"))
    }

    // ---------- windows ----------

    fn matching_windows(process: Option<&str>, title: Option<&str>, include_all: bool) -> Vec<isize> {
        let mut names: HashMap<u32, String> = HashMap::new();
        let mut found: Vec<isize> = win32::top_windows()
            .into_iter()
            .filter(|&h| {
                if win32::rect(h).is_none() && !win32::is_minimized(h) {
                    return false;
                }
                let t = win32::title(h);
                if !include_all && (t.trim().is_empty() || win32::is_tool_window(h)) {
                    return false;
                }
                // Cloaked but on this desktop: a suspended app frame nobody can see.
                if !include_all && win32::is_cloaked(h) && !win32::on_other_desktop(h) {
                    return false;
                }
                if let Some(want) = title
                    && !contains_ci(&t, want)
                {
                    return false;
                }
                if let Some(want) = process {
                    let pid = win32::pid_of(h);
                    let name = names.entry(pid).or_insert_with(|| win32::process_info(pid).0);
                    if !same_process(name, want) {
                        return false;
                    }
                }
                true
            })
            .collect();
        // Windows on this desktop first, front to back.
        found.sort_by_key(|&h| win32::is_cloaked(h));
        found
    }

    pub fn resolve(&self, w: &WindowArg) -> Result<isize> {
        if let Some(h) = w.hwnd {
            let h = h as isize;
            anyhow::ensure!(win32::exists(h), "hwnd {h} 창이 없음 (닫혔을 수 있음: list_windows로 다시 확인)");
            return Ok(h);
        }
        if w.is_empty() {
            bail!("창을 지정하세요: hwnd, 또는 process·title");
        }
        let found = Self::matching_windows(w.process.as_deref(), w.title.as_deref(), false);
        let found = if found.is_empty() { Self::matching_windows(w.process.as_deref(), w.title.as_deref(), true) } else { found };
        found.first().copied().ok_or_else(|| {
            anyhow!("맞는 창 없음 (process={:?}, title={:?}): list_windows로 확인하세요", w.process, w.title)
        })
    }

    fn window_line(h: isize) -> String {
        let pid = win32::pid_of(h);
        let (process, _) = win32::process_info(pid);
        let mut s = format!("hwnd={h} \"{}\" process={process} pid={pid} class={}", clip(&win32::title(h), 80), win32::stable_class(&win32::class_name(h)));
        if !win32::is_minimized(h)
            && let Some(r) = win32::rect(h)
        {
            let _ = write!(s, " {r}");
        }
        if win32::is_minimized(h) {
            s.push_str(" 최소화");
        } else if win32::is_maximized(h) {
            s.push_str(" 최대화");
        }
        if h == win32::foreground() {
            s.push_str(" 앞");
        }
        if win32::is_cloaked(h) && win32::on_other_desktop(h) {
            s.push_str(" 다른-가상-데스크톱");
        }
        if win32::out_of_reach(h) {
            s.push_str(" 관리자권한(이 서버가 일반 권한이라 조작 불가)");
        }
        s
    }

    pub fn screen_info(&mut self) -> Result<Reply> {
        let mut s = String::from("모니터 (좌표는 물리 픽셀):\n");
        for (i, m) in capture::monitors()?.iter().enumerate() {
            let _ = writeln!(s, "  {i}: {} 배율 {:.0}%{} {}", m.rect, m.scale * 100.0, if m.primary { " 주모니터" } else { "" }, m.name);
        }
        let (cx, cy) = input::cursor();
        let _ = writeln!(s, "마우스 위치: {cx},{cy}");
        let fg = win32::foreground();
        if fg != 0 {
            let _ = writeln!(s, "앞에 있는 창: {}", Self::window_line(fg));
        }
        let _ = writeln!(s, "이 서버 관리자 권한: {}", if win32::is_elevated(std::process::id()) { "예" } else { "아니오" });
        Ok(Reply::Text(s))
    }

    pub fn list_windows(&mut self, p: ListWindowsParams) -> Result<Reply> {
        let include_all = p.include_all.unwrap_or(false);
        let found = poll(p.wait_ms.unwrap_or(0), || {
            let w = Self::matching_windows(p.process.as_deref(), p.title.as_deref(), include_all);
            Ok((!w.is_empty()).then_some(w))
        })?;
        let Some(found) = found else {
            return Ok(Reply::Text("맞는 창 없음".into()));
        };
        let lines: Vec<String> = found.iter().map(|&h| Self::window_line(h)).collect();
        Ok(Reply::Text(format!("창 {}개 (앞쪽부터):\n{}", lines.len(), lines.join("\n"))))
    }

    pub fn window(&mut self, p: WindowParams) -> Result<Reply> {
        let h = self.resolve(&p.window)?;
        match p.action.as_str() {
            "focus" => {
                let front = input::focus_window(h);
                if !front {
                    bail!("앞으로 가져오지 못함 (Windows가 거부: 사용자가 다른 창을 쓰는 중일 수 있음)");
                }
            }
            "minimize" => win32::show(h, SW_MINIMIZE),
            "maximize" => win32::show(h, SW_MAXIMIZE),
            "restore" => win32::show(h, SW_RESTORE),
            "close" => {
                anyhow::ensure!(win32::close(h), "닫기 요청을 보내지 못함");
                sleep(Duration::from_millis(300));
                return Ok(Reply::Text(if win32::exists(h) {
                    "닫기를 요청했지만 창이 아직 있음 (저장 확인 창이 떴을 수 있음)".into()
                } else {
                    "닫힘".into()
                }));
            }
            "move" => {
                let cur = win32::rect(h).context("창 위치를 읽지 못함")?;
                let r = ScreenRect::new(
                    p.x.unwrap_or(cur.left),
                    p.y.unwrap_or(cur.top),
                    p.width.unwrap_or(cur.width()),
                    p.height.unwrap_or(cur.height()),
                );
                if win32::is_minimized(h) || win32::is_maximized(h) {
                    win32::show(h, SW_RESTORE);
                }
                anyhow::ensure!(win32::move_window(h, r), "창을 옮기지 못함");
            }
            other => bail!("알 수 없는 action: {other} (focus·minimize·maximize·restore·close·move)"),
        }
        sleep(Duration::from_millis(150));
        Ok(Reply::Text(Self::window_line(h)))
    }

    pub fn launch(&mut self, p: LaunchParams) -> Result<Reply> {
        let before: HashSet<isize> = win32::top_windows().into_iter().collect();
        let args = p.args.unwrap_or_default();
        let is_program = {
            let lower = p.path.to_lowercase();
            [".exe", ".com", ".bat", ".cmd"].iter().any(|e| lower.ends_with(e))
        };
        let pid = if is_program {
            let mut cmd = std::process::Command::new(&p.path);
            cmd.args(&args);
            if let Some(cwd) = &p.cwd {
                cmd.current_dir(cwd);
            }
            Some(cmd.spawn().with_context(|| format!("실행 실패: {}", p.path))?.id())
        } else {
            shell_open(&p.path, &args.join(" "), p.cwd.as_deref())?;
            None
        };
        let wait = p.wait_window_ms.unwrap_or(10_000);
        let mut s = match pid {
            Some(pid) => format!("실행함 pid={pid}"),
            None => format!("열기 요청함: {}", p.path),
        };
        if wait == 0 {
            return Ok(Reply::Text(s));
        }
        // A launcher may hand over to another process, so any new titled window counts, the
        // launched process's own first.
        let found = poll(wait, || {
            let mut new: Vec<isize> = Self::matching_windows(None, None, false).into_iter().filter(|h| !before.contains(h)).collect();
            new.sort_by_key(|&h| Some(win32::pid_of(h)) != pid);
            Ok((!new.is_empty()).then_some(new))
        })?;
        match found {
            Some(ws) => {
                s.push_str("\n새 창:\n");
                s.push_str(&ws.iter().map(|&h| Self::window_line(h)).collect::<Vec<_>>().join("\n"));
            }
            None => s.push_str(&format!("\n{wait}ms 안에 새 창이 뜨지 않음 (list_windows의 wait_ms로 더 기다릴 수 있음)")),
        }
        Ok(Reply::Text(s))
    }

    // ---------- screen ----------

    pub fn screenshot(&mut self, p: ScreenshotParams) -> Result<Reply> {
        let method = p.method.as_deref().unwrap_or("auto");
        let (img, origin, how): (RgbaImage, ScreenRect, String) = if !p.window.is_empty() {
            let h = self.resolve(&p.window)?;
            anyhow::ensure!(!win32::is_minimized(h), "창이 최소화되어 있음: window(action=restore) 후 다시 찍으세요");
            let by_window = || capture::capture_window(h).filter(|(img, _)| !capture::is_blank(img));
            let shot = match method {
                "window" => by_window().map(|(i, r)| (i, r, "창 그림".to_string())),
                "screen" => None,
                _ => by_window().map(|(i, r)| (i, r, "창 그림".to_string())),
            };
            match shot {
                Some(s) => s,
                None if method == "window" => bail!("창이 자기 그림을 그려 주지 않음: method=screen으로 찍으세요"),
                None => {
                    let r = win32::rect(h).context("창 위치를 읽지 못함")?;
                    let mut how = "화면에서 잘라냄".to_string();
                    if win32::is_cloaked(h) {
                        how.push_str(" (주의: 다른 가상 데스크톱 창이라 보이지 않을 수 있음)");
                    } else if win32::foreground() != h {
                        how.push_str(" (다른 창에 가려진 부분은 그 창이 찍힘)");
                    }
                    (capture::capture_rect(r)?, r, how)
                }
            }
        } else {
            let rect = if let Some(r) = &p.region {
                ScreenRect::new(r.x, r.y, r.width, r.height)
            } else if let Some(i) = p.monitor {
                capture::monitors()?.get(i).map(|m| m.rect).with_context(|| format!("모니터 {i} 없음 (screen_info로 확인)"))?
            } else if let Some(r) = &p.element {
                self.target_rect(r)?
            } else {
                capture::virtual_screen()?
            };
            (capture::capture_rect(rect)?, rect, "화면".to_string())
        };

        if let Some(path) = &p.save_path {
            img.save_with_format(path, image::ImageFormat::Png).with_context(|| format!("저장 실패: {path}"))?;
        }
        let max = p.max_size.unwrap_or(1568).max(64);
        let long = img.width().max(img.height());
        let scale = if long > max { max as f64 / long as f64 } else { 1.0 };
        let shown = if scale < 1.0 {
            let w = ((img.width() as f64 * scale).round() as u32).max(1);
            let h = ((img.height() as f64 * scale).round() as u32).max(1);
            imageops::resize(&img, w, h, imageops::FilterType::Triangle)
        } else {
            img.clone()
        };
        let mut text = format!(
            "{how}: 화면 {origin}, 이미지 {}x{}",
            shown.width(),
            shown.height()
        );
        if scale < 1.0 {
            let _ = write!(
                text,
                ", 배율 {scale:.4} → 화면 x = {} + 이미지x / {scale:.4}, 화면 y = {} + 이미지y / {scale:.4}",
                origin.left, origin.top
            );
        } else {
            let _ = write!(text, " (원본 크기) → 화면 x = {} + 이미지x, 화면 y = {} + 이미지y", origin.left, origin.top);
        }
        if let Some(path) = &p.save_path {
            let _ = write!(text, "\n원본 {}x{} 저장: {path}", img.width(), img.height());
        }
        Ok(Reply::Image { text, png: encode_png(&shown)? })
    }

    fn target_rect(&self, r: &str) -> Result<ScreenRect> {
        match self.target(r)? {
            Target::Uia(el) => uia::rect_of(el).context("요소의 영역을 읽지 못함 (사라졌거나 화면 밖)"),
            Target::Hwnd(h) => win32::rect(*h).context("컨트롤의 영역을 읽지 못함"),
        }
    }

    pub fn find_image(&mut self, p: FindImageParams) -> Result<Reply> {
        let tpl = image::open(&p.template_path).with_context(|| format!("그림을 열지 못함: {}", p.template_path))?.to_rgba8();
        anyhow::ensure!(matcher::usable(&tpl), "템플릿이 너무 밋밋함 (글자·테두리가 들어가게 다시 잘라 주세요)");
        let min = p.min_score.unwrap_or(0.85);
        let max = p.max_results.unwrap_or(5).max(1);
        let window = if p.window.is_empty() { None } else { Some(self.resolve(&p.window)?) };
        let found = poll(p.wait_ms.unwrap_or(0), || {
            let (hay, origin) = match window {
                Some(h) => match capture::capture_window(h).filter(|(i, _)| !capture::is_blank(i)) {
                    Some(s) => s,
                    None => {
                        let r = win32::rect(h).context("창 위치를 읽지 못함")?;
                        (capture::capture_rect(r)?, r)
                    }
                },
                None => {
                    let r = match &p.region {
                        Some(r) => ScreenRect::new(r.x, r.y, r.width, r.height),
                        None => capture::virtual_screen()?,
                    };
                    (capture::capture_rect(r)?, r)
                }
            };
            let mut all = matcher::candidates(&hay, &tpl, min);
            all.sort_by(|a, b| b.score.total_cmp(&a.score));
            all.truncate(max);
            Ok((!all.is_empty()).then(|| (all, origin)))
        })?;
        let Some((all, origin)) = found else {
            return Ok(Reply::Text(format!("못 찾음 (일치도 {min} 이상 없음)")));
        };
        let mut s = format!("{}곳 찾음 (화면 좌표, 일치도 높은 순):", all.len());
        for m in all {
            let (cx, cy) = m.center();
            let _ = write!(
                s,
                "\n  가운데 {},{} 영역 {} 일치도 {:.3}",
                origin.left + cx,
                origin.top + cy,
                ScreenRect::new(origin.left + m.x, origin.top + m.y, m.w as i32, m.h as i32),
                m.score
            );
        }
        Ok(Reply::Text(s))
    }

    // ---------- UI tree ----------

    fn uia_line(el: &UIElement) -> String {
        let name = el.get_name().unwrap_or_default();
        let mut s = format!("{} \"{}\"", uia::type_name(el), clip(&name, 80));
        let aid = el.get_automation_id().unwrap_or_default();
        // Win32 and WinForms report their changing window handle as the automation id.
        if !aid.is_empty() && aid.parse::<u64>().ok().is_none_or(|n| n <= 0xFFFF) {
            let _ = write!(s, " aid={aid}");
        }
        let class = el.get_classname().unwrap_or_default();
        if !class.is_empty() {
            let _ = write!(s, " class={}", win32::stable_class(&class));
        }
        if let Some(r) = uia::rect_of(el) {
            let _ = write!(s, " {r}");
        }
        if let Some(v) = uia::value_hint(el)
            && !v.is_empty()
            && v != name
        {
            let _ = write!(s, " value=\"{}\"", clip(&v, 80));
        }
        if !el.is_enabled().unwrap_or(true) {
            s.push_str(" 비활성");
        }
        if el.is_offscreen().unwrap_or(false) {
            s.push_str(" 화면밖");
        }
        s
    }

    /// The control's kind: what UIA calls it (custom WinForms buttons are plain windows by
    /// class), else a guess from the class.
    fn win32_kind(&self, h: isize) -> String {
        let class = win32::class_name(h);
        let guess = win32::kind_label(&class);
        if let Ok(uia) = self.uia()
            && let Some(el) = uia.from_hwnd(h)
        {
            let t = uia::type_name(&el);
            if t != "Pane" && t != "Unknown" || guess == "Control" {
                return t;
            }
        }
        guess.to_string()
    }

    fn win32_line(&self, h: isize) -> String {
        let class = win32::stable_class(&win32::class_name(h));
        let text = win32::strip_markup(&win32::text(h));
        let mut s = format!("{} \"{}\"", self.win32_kind(h), clip(&text, 80));
        if let Some(n) = win32::control_name(h) {
            let _ = write!(s, " name={n}");
        }
        let _ = write!(s, " class={class}");
        if let Some(r) = win32::rect(h) {
            let _ = write!(s, " {r}");
        }
        if !win32::is_enabled(h) {
            s.push_str(" 비활성");
        }
        s
    }

    /// The element to start from: an explicit ref, or the window.
    fn root_target(&mut self, window: &WindowArg, element: Option<&str>, backend: &str) -> Result<Target> {
        if let Some(r) = element {
            return Ok(match self.target(r)? {
                Target::Uia(el) => Target::Uia(el.clone()),
                Target::Hwnd(h) => Target::Hwnd(*h),
            });
        }
        let h = self.resolve(window)?;
        if win32::out_of_reach(h) {
            bail!("관리자 권한으로 실행된 창이라 일반 권한인 이 서버가 들여다볼 수 없음 (서버를 관리자 권한으로 실행해야 함)");
        }
        Ok(match backend {
            "win32" => Target::Hwnd(h),
            _ => Target::Uia(self.uia()?.from_hwnd(h).context("창의 UI Automation 요소를 얻지 못함")?),
        })
    }

    pub fn ui_tree(&mut self, p: TreeParams) -> Result<Reply> {
        let backend = p.backend.as_deref().unwrap_or("uia");
        let root = self.root_target(&p.window, p.element.as_deref(), backend)?;
        let max_depth = p.depth.unwrap_or(8).max(1);
        let mut budget = p.max_nodes.unwrap_or(300).max(1) as usize;
        let offscreen = p.include_offscreen.unwrap_or(false);
        let mut out = Vec::new();
        let mut cut = 0usize;
        match root {
            Target::Uia(el) => {
                let r = self.add_ref(Target::Uia(el.clone()));
                out.push(format!("{r} {}", Self::uia_line(&el)));
                self.walk_uia(&el, 1, max_depth, &mut budget, offscreen, &mut out, &mut cut)?;
            }
            Target::Hwnd(h) => {
                let r = self.add_ref(Target::Hwnd(h));
                out.push(format!("{r} {}", self.win32_line(h)));
                self.walk_win32(h, 1, max_depth, &mut budget, &mut out, &mut cut);
            }
        }
        let mut s = out.join("\n");
        if cut > 0 {
            let _ = write!(s, "\n… {cut}개 이상 생략 (max_nodes 초과: 범위를 ref로 좁히거나 ui_find를 쓰세요)");
        }
        let other_desktop = p.element.is_none()
            && backend == "uia"
            && self.resolve(&p.window).is_ok_and(|h| win32::is_cloaked(h) && win32::on_other_desktop(h));
        if other_desktop {
            s.push_str("\n(다른 가상 데스크톱 창: UIA는 제목 줄 정도만 보여 줌. 안쪽 컨트롤은 backend=win32로 보세요)");
        } else if out.len() <= 1 && backend == "uia" {
            s.push_str("\n(자식 요소가 없음: 다른 가상 데스크톱 창이면 UIA가 일부만 보여 줌. backend=win32로 시도하거나 screenshot을 쓰세요)");
        }
        Ok(Reply::Text(s))
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_uia(
        &mut self,
        el: &UIElement,
        depth: u32,
        max_depth: u32,
        budget: &mut usize,
        offscreen: bool,
        out: &mut Vec<String>,
        cut: &mut usize,
    ) -> Result<()> {
        let children = self.uia()?.children(el);
        for c in children {
            if !offscreen && !uia::is_visible(&c) {
                continue;
            }
            if *budget == 0 {
                *cut += 1;
                continue;
            }
            *budget -= 1;
            let line = Self::uia_line(&c);
            let r = self.add_ref(Target::Uia(c.clone()));
            let indent = "  ".repeat(depth as usize);
            if depth < max_depth {
                out.push(format!("{indent}{r} {line}"));
                self.walk_uia(&c, depth + 1, max_depth, budget, offscreen, out, cut)?;
            } else {
                let more = self.uia()?.walker.get_first_child(&c).is_ok();
                out.push(format!("{indent}{r} {line}{}", if more { " ▸(더 있음)" } else { "" }));
            }
        }
        Ok(())
    }

    fn walk_win32(&mut self, h: isize, depth: u32, max_depth: u32, budget: &mut usize, out: &mut Vec<String>, cut: &mut usize) {
        for c in win32::children(h) {
            if !win32::is_visible(c) {
                continue;
            }
            if *budget == 0 {
                *cut += 1;
                continue;
            }
            *budget -= 1;
            let line = self.win32_line(c);
            let r = self.add_ref(Target::Hwnd(c));
            let indent = "  ".repeat(depth as usize);
            if depth < max_depth {
                out.push(format!("{indent}{r} {line}"));
                self.walk_win32(c, depth + 1, max_depth, budget, out, cut);
            } else {
                let more = !win32::children(c).is_empty();
                out.push(format!("{indent}{r} {line}{}", if more { " ▸(더 있음)" } else { "" }));
            }
        }
    }

    pub fn ui_find(&mut self, p: FindParams) -> Result<Reply> {
        let backend = p.backend.clone().unwrap_or_else(|| "uia".into());
        let root = self.root_target(&p.window, p.under.as_deref(), &backend)?;
        let max = p.max_results.unwrap_or(30).max(1) as usize;
        let offscreen = p.include_offscreen.unwrap_or(false);
        let found = match root {
            Target::Uia(el) => {
                let uia = self.uia()?;
                poll(p.wait_ms.unwrap_or(0), || {
                    // Conditions UIA checks inside the other program, so only matches come back.
                    let mut cond = uia.auto.create_true_condition()?;
                    if let Some(aid) = &p.automation_id {
                        let c = uia.auto.create_property_condition(UIProperty::AutomationId, Variant::from(aid.as_str()), None)?;
                        cond = uia.auto.create_and_condition(cond, c)?;
                    }
                    if let Some(t) = &p.control_type {
                        let id = uia::control_type_id(t).with_context(|| format!("알 수 없는 control_type: {t}"))?;
                        let c = uia.auto.create_property_condition(UIProperty::ControlType, Variant::from(id), None)?;
                        cond = uia.auto.create_and_condition(cond, c)?;
                    }
                    let all = el.find_all(TreeScope::Descendants, &cond).unwrap_or_default();
                    let hits: Vec<UIElement> = all
                        .into_iter()
                        .filter(|e| {
                            p.control_type.as_ref().is_none_or(|t| uia::type_name(e).eq_ignore_ascii_case(t))
                                && p.name.as_ref().is_none_or(|n| contains_ci(&e.get_name().unwrap_or_default(), n))
                                && p.class_name.as_ref().is_none_or(|c| contains_ci(&e.get_classname().unwrap_or_default(), c))
                                && p.value.as_ref().is_none_or(|v| uia::value_hint(e).is_some_and(|x| contains_ci(&x, v)))
                                && (offscreen || uia::is_visible(e))
                        })
                        .take(max + 1)
                        .collect();
                    Ok((!hits.is_empty()).then_some(hits))
                })?
                .map(|hits| hits.into_iter().map(Target::Uia).collect::<Vec<_>>())
            }
            Target::Hwnd(h) => poll(p.wait_ms.unwrap_or(0), || {
                let mut all = Vec::new();
                fn collect(h: isize, out: &mut Vec<isize>) {
                    for c in win32::children(h) {
                        if win32::is_visible(c) {
                            out.push(c);
                            collect(c, out);
                        }
                    }
                }
                collect(h, &mut all);
                let hits: Vec<isize> = all
                    .into_iter()
                    .filter(|&c| {
                        let class = win32::class_name(c);
                        p.control_type.as_ref().is_none_or(|t| self.win32_kind(c).eq_ignore_ascii_case(t))
                            && p.class_name.as_ref().is_none_or(|n| contains_ci(&class, n))
                            && p.name.as_ref().is_none_or(|n| contains_ci(&win32::strip_markup(&win32::text(c)), n))
                            && p.value.as_ref().is_none_or(|v| contains_ci(&win32::text(c), v))
                            && p.automation_id.as_ref().is_none_or(|a| win32::control_name(c).as_deref() == Some(a.as_str()))
                    })
                    .take(max + 1)
                    .collect();
                Ok((!hits.is_empty()).then_some(hits))
            })?
            .map(|hits| hits.into_iter().map(Target::Hwnd).collect()),
        };
        let Some(mut hits) = found else {
            let mut s = "못 찾음".to_string();
            if backend == "uia" {
                s.push_str(" (UIA에 안 보이는 앱이면 backend=win32, 그래도 없으면 screenshot·find_image를 쓰세요)");
            }
            return Ok(Reply::Text(s));
        };
        let more = hits.len() > max;
        hits.truncate(max);
        let mut lines = Vec::new();
        for t in hits {
            let line = match &t {
                Target::Uia(e) => Self::uia_line(e),
                Target::Hwnd(h) => self.win32_line(*h),
            };
            let r = self.add_ref(t);
            lines.push(format!("{r} {line}"));
        }
        let mut s = format!("{}개:\n{}", lines.len(), lines.join("\n"));
        if more {
            s.push_str("\n… 더 있음 (조건을 좁히거나 max_results를 늘리세요)");
        }
        Ok(Reply::Text(s))
    }

    pub fn ui_at(&mut self, p: AtParams) -> Result<Reply> {
        let window = win32::top_window_at(p.x, p.y).map(win32::root_of);
        if let Some(w) = window {
            self.check_reach(w)?;
        }
        let target = match p.backend.as_deref().unwrap_or("uia") {
            "win32" => Target::Hwnd(win32::deepest_at(p.x, p.y).context("그 자리에 창이 없음")?),
            _ => Target::Uia(self.uia()?.element_at(p.x, p.y).context("그 자리의 요소를 얻지 못함")?),
        };
        let line = match &target {
            Target::Uia(e) => Self::uia_line(e),
            Target::Hwnd(h) => self.win32_line(*h),
        };
        let r = self.add_ref(target);
        let mut s = format!("{r} {line}");
        if let Some(w) = window {
            let _ = write!(s, "
창: {}", Self::window_line(w));
        }
        Ok(Reply::Text(s))
    }

    pub fn ui_read(&mut self, p: ReadParams) -> Result<Reply> {
        let max = p.max_chars.unwrap_or(20_000);
        let v = match self.target(&p.element)? {
            Target::Uia(el) => uia::read_value(el).context("값을 읽지 못함 (요소가 사라졌을 수 있음)")?,
            Target::Hwnd(h) => {
                anyhow::ensure!(win32::exists(*h), "컨트롤이 사라짐");
                win32::text(*h)
            }
        };
        let v = unify_newlines(&v);
        let total = v.chars().count();
        Ok(Reply::Text(if total > max {
            format!("{}\n… (전체 {total}자 중 {max}자)", v.chars().take(max).collect::<String>())
        } else {
            v
        }))
    }

    /// Brings the element's window forward and puts the keyboard focus on it.
    fn focus_target(&self, r: &str) -> Result<()> {
        match self.target(r)? {
            Target::Uia(el) => {
                if let Some(w) = self.uia()?.window_of(el) {
                    self.check_reach(w)?;
                    input::focus_window(w);
                }
                el.set_focus().map_err(|e| anyhow!("포커스를 주지 못함: {}", e.message()))?;
            }
            Target::Hwnd(h) => {
                let w = win32::root_of(*h);
                self.check_reach(w)?;
                input::focus_window(w);
                let el = self.uia()?.from_hwnd(*h).context("컨트롤이 사라짐")?;
                el.set_focus().map_err(|e| anyhow!("포커스를 주지 못함: {}", e.message()))?;
            }
        }
        sleep(Duration::from_millis(80));
        Ok(())
    }

    fn check_reach(&self, w: isize) -> Result<()> {
        anyhow::ensure!(!win32::out_of_reach(w), "관리자 권한 창이라 일반 권한인 이 서버의 입력이 막힘");
        Ok(())
    }

    fn read_back(&self, r: &str) -> Option<String> {
        match self.target(r).ok()? {
            Target::Uia(el) => uia::read_value(el),
            Target::Hwnd(h) => Some(win32::text(*h)),
        }
    }

    pub fn ui_set_value(&mut self, p: SetValueParams) -> Result<Reply> {
        let method = p.method.as_deref().unwrap_or("auto");
        let mut how = None;
        let mut why_not = String::new();
        if method != "type" {
            let direct = match self.target(&p.element)? {
                Target::Uia(el) => uia::set_value(el, &p.value).map(|_| "값 직접 설정"),
                Target::Hwnd(h) => {
                    self.check_reach(win32::root_of(*h))?;
                    if win32::set_text(*h, &p.value) { Ok("WM_SETTEXT") } else { Err("WM_SETTEXT 실패".into()) }
                }
            };
            match direct {
                Ok(h) => how = Some(h),
                Err(e) if method == "direct" => bail!("값을 직접 설정하지 못함: {e}"),
                Err(e) => why_not = e,
            }
        }
        if how.is_none() {
            self.focus_target(&p.element)?;
            input::key_combo("Ctrl+A")?;
            sleep(Duration::from_millis(40));
            if p.value.is_empty() {
                input::key_combo("Delete")?;
            } else {
                input::type_text(&p.value);
            }
            how = Some("타이핑");
        }
        sleep(Duration::from_millis(120));
        let now = self.read_back(&p.element).unwrap_or_default();
        let mut s = format!("{}함", how.unwrap_or_default());
        if !why_not.is_empty() {
            let _ = write!(s, " (직접 설정 안 됨: {why_not})");
        }
        let _ = write!(s, " → 지금 값: \"{}\"", clip(&now, 200));
        if unify_newlines(&now).trim_end() != unify_newlines(&p.value).trim_end() {
            s.push_str("\n주의: 넣은 값과 다름 (형식이 바뀌었거나 입력이 막혔을 수 있음. screenshot으로 확인하세요)");
        }
        Ok(Reply::Text(s))
    }

    pub fn ui_click(&mut self, p: ClickParams) -> Result<Reply> {
        let kind = ClickKind::parse(p.button.as_deref().unwrap_or("left"))?;
        let method = p.method.as_deref().unwrap_or("auto");
        if method != "mouse" && kind == ClickKind::Left {
            let pressed = match self.target(&p.element)? {
                Target::Uia(el) => uia::press(el),
                Target::Hwnd(h) => {
                    self.check_reach(win32::root_of(*h))?;
                    let r = win32::press(*h).map(|_| "BM_CLICK");
                    if r.is_ok() {
                        win32::settle(*h);
                    }
                    r
                }
            };
            match pressed {
                Ok(how) => return Ok(Reply::Text(format!("눌렀음 (마우스 없이: {how})"))),
                Err(NotPressed::Disabled) => bail!("비활성 상태라 누를 수 없음 (화면이 아직 준비 중일 수 있음: 잠시 뒤 다시)"),
                Err(NotPressed::Gone) => bail!("요소가 사라짐: ui_find로 다시 찾으세요"),
                Err(NotPressed::Unknown(m)) => {
                    return Ok(Reply::Text(format!("누르는 중 오류 ({m}): 눌렸을 수도 있으니 다시 누르기 전에 screenshot으로 확인하세요")));
                }
                Err(NotPressed::Refused(why)) if method == "pattern" => bail!("마우스 없이 누를 수 없음: {why}"),
                Err(NotPressed::Refused(_)) => {}
            }
        }
        // The mouse, at the element's centre, with its window in front.
        let (rect, window) = match self.target(&p.element)? {
            Target::Uia(el) => (uia::rect_of(el).context("요소 영역을 읽지 못함 (사라졌거나 화면 밖)")?, self.uia()?.window_of(el)),
            Target::Hwnd(h) => (win32::rect(*h).context("컨트롤 영역을 읽지 못함")?, Some(win32::root_of(*h))),
        };
        if let Some(w) = window {
            self.check_reach(w)?;
            input::focus_window(w);
        }
        let (x, y) = rect.center();
        let mut s = String::new();
        if let Some(w) = window
            && win32::top_window_at(x, y).is_some_and(|top| win32::root_of(top) != w)
        {
            s.push_str("주의: 그 자리에 다른 창이 위에 있음. ");
        }
        input::click_at(x, y, kind);
        let _ = write!(s, "마우스로 {kind:?} 클릭: {x},{y}");
        Ok(Reply::Text(s))
    }

    // ---------- input ----------

    pub fn mouse(&mut self, p: MouseParams) -> Result<Reply> {
        let action = p.action.as_deref().unwrap_or("click");
        if let Some(top) = win32::top_window_at(p.x, p.y) {
            self.check_reach(win32::root_of(top))?;
        }
        match action {
            "move" => input::move_to(p.x, p.y),
            "scroll" => input::scroll(p.x, p.y, p.notches.unwrap_or(-3), p.horizontal.unwrap_or(false)),
            "drag" => {
                let to = (p.to_x.context("drag에는 to_x가 필요")?, p.to_y.context("drag에는 to_y가 필요")?);
                input::drag((p.x, p.y), to);
            }
            "click" => input::click_at(p.x, p.y, ClickKind::Left),
            other => input::click_at(p.x, p.y, ClickKind::parse(other)?),
        }
        let under = win32::top_window_at(p.x, p.y).map(|h| format!(" (그 자리 창: \"{}\")", clip(&win32::title(h), 60))).unwrap_or_default();
        Ok(Reply::Text(format!("{action} {},{}{under}", p.x, p.y)))
    }

    pub fn type_text(&mut self, p: TypeParams) -> Result<Reply> {
        if let Some(r) = &p.element {
            self.focus_target(r)?;
        } else if !p.window.is_empty() {
            let h = self.resolve(&p.window)?;
            self.check_reach(h)?;
            input::focus_window(h);
        }
        input::type_text(&p.text);
        Ok(Reply::Text(format!("{}자 입력함", p.text.chars().count())))
    }

    pub fn press_keys(&mut self, p: KeysParams) -> Result<Reply> {
        if !p.window.is_empty() {
            let h = self.resolve(&p.window)?;
            self.check_reach(h)?;
            input::focus_window(h);
        }
        let delay = Duration::from_millis(p.delay_ms.unwrap_or(80));
        for (i, k) in p.keys.iter().enumerate() {
            if i > 0 {
                sleep(delay);
            }
            input::key_combo(k)?;
        }
        Ok(Reply::Text(format!("눌렀음: {}", p.keys.join(", "))))
    }

    pub fn clipboard(&mut self, p: ClipboardParams) -> Result<Reply> {
        let mut cb = arboard::Clipboard::new().context("클립보드를 열지 못함")?;
        match p.action.as_deref().unwrap_or("get") {
            "get" => Ok(Reply::Text(cb.get_text().unwrap_or_else(|_| "(글자가 아닌 내용이거나 비어 있음)".into()))),
            "set" => {
                let t = p.text.context("set에는 text가 필요")?;
                cb.set_text(t.clone()).context("클립보드에 쓰지 못함")?;
                Ok(Reply::Text(format!("클립보드에 {}자 넣음", t.chars().count())))
            }
            other => bail!("알 수 없는 action: {other} (get·set)"),
        }
    }
}

fn shell_open(path: &str, args: &str, cwd: Option<&str>) -> Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR, w};
    let file = HSTRING::from(path);
    let params = HSTRING::from(args);
    let dir = cwd.map(HSTRING::from);
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &file,
            if args.is_empty() { PCWSTR::null() } else { PCWSTR(params.as_ptr()) },
            dir.as_ref().map(|d| PCWSTR(d.as_ptr())).unwrap_or(PCWSTR::null()),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    anyhow::ensure!(r.0 as isize > 32, "열기 실패: {path} (코드 {})", r.0 as isize);
    Ok(())
}

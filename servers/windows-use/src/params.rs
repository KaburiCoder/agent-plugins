//! Tool inputs. Field docs become the JSON schema descriptions the model reads.

use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

/// Which top-level window. `hwnd` wins; otherwise the front-most window matching both
/// `process` and `title`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WindowArg {
    /// 창 핸들 (list_windows의 hwnd 값)
    pub hwnd: Option<i64>,
    /// 프로세스 실행 파일 이름 (예: "notepad.exe", ".exe"는 생략 가능, 대소문자 무시)
    pub process: Option<String>,
    /// 창 제목에 들어 있는 글자 (대소문자 무시)
    pub title: Option<String>,
}

impl WindowArg {
    pub fn is_empty(&self) -> bool {
        self.hwnd.is_none() && self.process.is_none() && self.title.is_none()
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Empty {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListWindowsParams {
    /// 이 프로세스의 창만 (예: "chrome.exe")
    pub process: Option<String>,
    /// 제목에 이 글자가 들어간 창만
    pub title: Option<String>,
    /// 제목 없는 창과 도구 창까지 모두 보기 (기본 false)
    pub include_all: Option<bool>,
    /// 맞는 창이 없으면 생길 때까지 기다릴 시간(ms). 늦게 뜨는 창을 기다릴 때 사용
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WindowParams {
    #[serde(flatten)]
    pub window: WindowArg,
    /// focus(앞으로) · minimize · maximize · restore · close(닫기 요청) · move(x,y,width,height 필요)
    pub action: String,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScreenshotParams {
    /// 찍을 창. 비우면 region·monitor·ref, 그것도 없으면 모든 모니터 전체
    #[serde(flatten)]
    pub window: WindowArg,
    /// 이 화면 영역만 (물리 픽셀 화면 좌표)
    pub region: Option<Region>,
    /// 이 모니터만 (screen_info의 번호)
    pub monitor: Option<usize>,
    /// 이 요소(ui_tree·ui_find의 ref)가 차지한 영역만
    #[serde(rename = "ref")]
    pub element: Option<String>,
    /// 돌려줄 이미지의 긴 변 최대 픽셀 (기본 1568). 원본이 크면 줄여서 보냄
    pub max_size: Option<u32>,
    /// 원본 해상도 PNG를 이 경로에 저장 (find_image 템플릿 만들기에 사용)
    pub save_path: Option<String>,
    /// 창을 찍을 때: auto(기본, 창 자체 그림 → 실패하면 화면) · window(가려져도 창 그림) · screen(화면에 보이는 그대로)
    pub method: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TreeParams {
    #[serde(flatten)]
    pub window: WindowArg,
    /// 이 요소 아래만 펼침 (앞서 받은 ref)
    #[serde(rename = "ref")]
    pub element: Option<String>,
    /// uia(기본) · win32(자식 창 핸들 트리. UIA가 막혔거나 빈약한 WinForms·Win32·Delphi 앱용)
    pub backend: Option<String>,
    /// 펼칠 깊이 (기본 8)
    pub depth: Option<u32>,
    /// 최대 요소 수 (기본 300)
    pub max_nodes: Option<u32>,
    /// 화면에 안 보이는 요소도 포함 (기본 false)
    pub include_offscreen: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindParams {
    #[serde(flatten)]
    pub window: WindowArg,
    /// 이 요소 아래에서만 찾음 (앞서 받은 ref)
    #[serde(rename = "ref")]
    pub under: Option<String>,
    /// 이름(표시 글자)에 이 글자가 들어감 (대소문자 무시)
    pub name: Option<String>,
    /// AutomationId가 정확히 이것 (win32 backend에선 WinForms 컨트롤 Name)
    pub automation_id: Option<String>,
    /// 컨트롤 종류 (Button, Edit, Text, ComboBox, ListItem, DataItem, TabItem, MenuItem, CheckBox, Document, Pane, Window …)
    pub control_type: Option<String>,
    /// 클래스 이름에 이 글자가 들어감
    pub class_name: Option<String>,
    /// 값(입력칸 내용)에 이 글자가 들어감
    pub value: Option<String>,
    /// uia(기본) · win32
    pub backend: Option<String>,
    /// 최대 결과 수 (기본 30)
    pub max_results: Option<u32>,
    /// 화면에 안 보이는 요소도 포함 (기본 false)
    pub include_offscreen: Option<bool>,
    /// 못 찾으면 나타날 때까지 기다릴 시간(ms)
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadParams {
    /// ui_tree·ui_find가 준 ref (예: "e12")
    #[serde(rename = "ref")]
    pub element: String,
    /// 최대 글자 수 (기본 20000)
    pub max_chars: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetValueParams {
    /// ui_tree·ui_find가 준 ref
    #[serde(rename = "ref")]
    pub element: String,
    /// 넣을 값 (기존 내용을 바꿈)
    pub value: String,
    /// auto(기본: 값 직접 설정 → 안 되면 타이핑) · direct(값 직접 설정만) · type(포커스 후 전체 선택하고 타이핑)
    pub method: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClickParams {
    /// ui_tree·ui_find가 준 ref
    #[serde(rename = "ref")]
    pub element: String,
    /// left(기본) · double · right · middle
    pub button: Option<String>,
    /// auto(기본: 마우스 없이 누르기 → 거부되면 마우스) · pattern(마우스 없이만) · mouse(요소 가운데를 마우스로)
    pub method: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MouseParams {
    /// click(기본) · double · right · middle · move · scroll · drag
    pub action: Option<String>,
    /// 화면 좌표 x (물리 픽셀)
    pub x: i32,
    /// 화면 좌표 y (물리 픽셀)
    pub y: i32,
    /// drag의 끝점 x
    pub to_x: Option<i32>,
    /// drag의 끝점 y
    pub to_y: Option<i32>,
    /// scroll 눈금 수: 양수는 위(가로면 오른쪽), 음수는 아래 (기본 -3)
    pub notches: Option<i32>,
    /// 가로 스크롤 (기본 false)
    pub horizontal: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TypeParams {
    /// 입력할 글자. 한글·기호도 키보드 배열과 한/영 상태에 상관없이 그대로 들어감. \n은 Enter, \t는 Tab
    pub text: String,
    /// 먼저 이 창을 앞으로 가져옴
    #[serde(flatten)]
    pub window: WindowArg,
    /// 먼저 이 요소에 포커스를 줌 (ref)
    #[serde(rename = "ref")]
    pub element: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeysParams {
    /// 차례로 누를 키 조합들. 예: ["Ctrl+A", "Delete"], ["Alt+F4"], ["Enter"], ["Win+R"]
    pub keys: Vec<String>,
    /// 먼저 이 창을 앞으로 가져옴
    #[serde(flatten)]
    pub window: WindowArg,
    /// 조합 사이 간격 ms (기본 80)
    pub delay_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindImageParams {
    /// 찾을 그림 PNG 파일 경로 (screenshot의 save_path로 만들 수 있음)
    pub template_path: String,
    /// 이 창 안에서만 찾음
    #[serde(flatten)]
    pub window: WindowArg,
    /// 이 화면 영역 안에서만
    pub region: Option<Region>,
    /// 최소 일치도 0~1 (기본 0.85)
    pub min_score: Option<f32>,
    /// 최대 결과 수 (기본 5)
    pub max_results: Option<usize>,
    /// 못 찾으면 나타날 때까지 기다릴 시간(ms)
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClipboardParams {
    /// get(기본) · set
    pub action: Option<String>,
    /// set할 글자
    pub text: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LaunchParams {
    /// 실행 파일 경로·이름(예: "notepad.exe"), 또는 문서·폴더·URL (연결된 프로그램으로 열림)
    pub path: String,
    /// 명령줄 인수
    pub args: Option<Vec<String>>,
    /// 작업 폴더
    pub cwd: Option<String>,
    /// 새 창이 뜰 때까지 기다릴 시간(ms, 기본 10000, 0이면 안 기다림)
    pub wait_window_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AtParams {
    /// 화면 좌표 x (물리 픽셀)
    pub x: i32,
    /// 화면 좌표 y (물리 픽셀)
    pub y: i32,
    /// uia(기본) · win32
    pub backend: Option<String>,
}

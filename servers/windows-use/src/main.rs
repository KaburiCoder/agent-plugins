//! windows-use: an MCP server (stdio) that sees, reads and drives Windows desktop programs.
//! UI Automation first, the Win32 control tree for programs UIA cannot see into, and
//! screenshots with image matching for everything else.

mod capture;
mod desktop;
mod geom;
mod input;
mod matcher;
mod params;
mod uia;
mod win32;

use base64::Engine as _;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router, transport::stdio};

use desktop::{Desktop, Reply, State};
use params::*;

#[derive(Clone)]
struct WindowsUse {
    desktop: Desktop,
}

type ToolResult = Result<CallToolResult, ErrorData>;

impl WindowsUse {
    async fn call(&self, f: impl FnOnce(&mut State) -> anyhow::Result<Reply> + Send + 'static) -> ToolResult {
        let result = self.desktop.run(f).await.and_then(|r| r);
        Ok(match result {
            Ok(Reply::Text(t)) => CallToolResult::success(vec![ContentBlock::text(t)]),
            Ok(Reply::Image { text, png }) => CallToolResult::success(vec![
                ContentBlock::text(text),
                ContentBlock::image(base64::engine::general_purpose::STANDARD.encode(png), "image/png"),
            ]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("{e:#}"))]),
        })
    }
}

#[tool_router]
impl WindowsUse {
    #[tool(description = "모니터 배치·배율, 마우스 위치, 앞에 있는 창, 이 서버의 관리자 권한 여부. 좌표는 모두 물리 픽셀 화면 좌표.")]
    async fn screen_info(&self, Parameters(_): Parameters<Empty>) -> ToolResult {
        self.call(|s| s.screen_info()).await
    }

    #[tool(description = "열린 최상위 창 목록(앞쪽부터): hwnd, 제목, 프로세스, 위치, 최소화·다른 가상 데스크톱·관리자 권한 표시. process·title로 거르고 wait_ms로 늦게 뜨는 창을 기다림.")]
    async fn list_windows(&self, Parameters(p): Parameters<ListWindowsParams>) -> ToolResult {
        self.call(move |s| s.list_windows(p)).await
    }

    #[tool(description = "창 다루기: focus(앞으로 가져오기, 최소화면 복원) · minimize · maximize · restore · close(닫기 요청) · move(x,y,width,height).")]
    async fn window(&self, Parameters(p): Parameters<WindowParams>) -> ToolResult {
        self.call(move |s| s.window(p)).await
    }

    #[tool(description = "프로그램 실행 또는 문서·폴더·URL 열기. 새로 뜬 창의 hwnd를 돌려줌.")]
    async fn launch(&self, Parameters(p): Parameters<LaunchParams>) -> ToolResult {
        self.call(move |s| s.launch(p)).await
    }

    #[tool(description = "스크린샷: 창(가려져 있어도 창 자체 그림)·영역·모니터·요소(ref)·전체 화면. 이미지와 함께 이미지 좌표를 화면 좌표로 바꾸는 식을 줌. save_path로 원본 PNG 저장(find_image 템플릿용).")]
    async fn screenshot(&self, Parameters(p): Parameters<ScreenshotParams>) -> ToolResult {
        self.call(move |s| s.screenshot(p)).await
    }

    #[tool(description = "창(또는 ref 요소) 아래 UI 요소 트리. 줄마다 ref·종류·이름·aid·영역·값이 나오고, ref로 ui_read·ui_set_value·ui_click을 함. UIA가 비면 backend=win32.")]
    async fn ui_tree(&self, Parameters(p): Parameters<TreeParams>) -> ToolResult {
        self.call(move |s| s.ui_tree(p)).await
    }

    #[tool(description = "창 안에서 조건(name 포함·automation_id·control_type·class_name·value)에 맞는 요소 찾기. ref를 돌려줌. wait_ms로 나타날 때까지 기다림.")]
    async fn ui_find(&self, Parameters(p): Parameters<FindParams>) -> ToolResult {
        self.call(move |s| s.ui_find(p)).await
    }

    #[tool(description = "화면 좌표(x,y)에 있는 UI 요소와 그 창을 ref로 돌려줌. 스크린샷에서 본 것을 ui_read·ui_click으로 다룰 때 사용.")]
    async fn ui_at(&self, Parameters(p): Parameters<AtParams>) -> ToolResult {
        self.call(move |s| s.ui_at(p)).await
    }

    #[tool(description = "요소(ref)의 값·글자 전체 읽기 (입력칸 값, 문서 본문, 라벨 글자).")]
    async fn ui_read(&self, Parameters(p): Parameters<ReadParams>) -> ToolResult {
        self.call(move |s| s.ui_read(p)).await
    }

    #[tool(description = "요소(ref)에 값 넣기: 먼저 값을 직접 설정하고, 안 되면 포커스 후 전체 선택해 타이핑. 넣은 뒤 실제 값을 다시 읽어 알려 줌.")]
    async fn ui_set_value(&self, Parameters(p): Parameters<SetValueParams>) -> ToolResult {
        self.call(move |s| s.ui_set_value(p)).await
    }

    #[tool(description = "요소(ref) 누르기: 먼저 마우스 없이(Invoke·Toggle·Select·BM_CLICK), 거부되면 창을 앞으로 가져와 가운데를 마우스로 클릭. double·right는 마우스.")]
    async fn ui_click(&self, Parameters(p): Parameters<ClickParams>) -> ToolResult {
        self.call(move |s| s.ui_click(p)).await
    }

    #[tool(description = "화면 좌표에 마우스 동작: click · double · right · middle · move · scroll(notches) · drag(to_x,to_y). 좌표는 물리 픽셀.")]
    async fn mouse(&self, Parameters(p): Parameters<MouseParams>) -> ToolResult {
        self.call(move |s| s.mouse(p)).await
    }

    #[tool(description = "글자 입력 (한글 포함, 한/영 상태와 무관). 창이나 ref를 주면 먼저 그쪽에 포커스.")]
    async fn type_text(&self, Parameters(p): Parameters<TypeParams>) -> ToolResult {
        self.call(move |s| s.type_text(p)).await
    }

    #[tool(description = "키 조합을 차례로 누르기. 예: [\"Ctrl+S\"], [\"Ctrl+A\",\"Delete\"], [\"Alt+F4\"], [\"Win+R\"]. 창을 주면 먼저 앞으로.")]
    async fn press_keys(&self, Parameters(p): Parameters<KeysParams>) -> ToolResult {
        self.call(move |s| s.press_keys(p)).await
    }

    #[tool(description = "그림(PNG 템플릿)이 화면·창·영역 어디 있는지 찾기. 크기가 조금 달라도 찾음. 가운데 화면 좌표와 일치도를 돌려줌. wait_ms로 나타날 때까지 기다림.")]
    async fn find_image(&self, Parameters(p): Parameters<FindImageParams>) -> ToolResult {
        self.call(move |s| s.find_image(p)).await
    }

    #[tool(description = "클립보드 글자 읽기(get)·쓰기(set).")]
    async fn clipboard(&self, Parameters(p): Parameters<ClipboardParams>) -> ToolResult {
        self.call(move |s| s.clipboard(p)).await
    }
}

#[tool_handler(
    name = "windows-use",
    instructions = "Windows 데스크톱 프로그램을 보고·읽고·조작한다(사용자의 실제 PC).\n\
순서: list_windows로 창 찾기(늦게 뜨면 wait_ms) → ui_find(먼저)나 ui_tree로 요소와 ref 얻기 → ui_read·ui_set_value·ui_click → 결과 문구나 작은 screenshot으로 확인.\n\
수단: UIA → 비거나 빈약하면 backend=win32(다른 가상 데스크톱 창도) → screenshot 보고 ui_at → mouse 좌표 클릭 → find_image.\n\
좌표는 모두 물리 픽셀 화면 좌표. screenshot 이미지 좌표는 결과에 나오는 식으로 화면 좌표로 바꿔 쓴다.\n\
글자 입력은 type_text(한/영 상태 무관), press_keys는 단축키에만.\n\
ui_click이 '눌렸을 수도 있으니'를 돌려주면 다시 누르지 말고 화면부터 확인한다.\n\
ref는 그때의 요소다. 화면이 바뀌면 다시 찾는다.\n\
삭제·전송·저장 안 한 문서 닫기 같은 되돌리기 어려운 동작은 먼저 사용자에게 확인하고, 비밀번호는 읽거나 입력하지 않는다."
)]
impl ServerHandler for WindowsUse {}

fn main() -> anyhow::Result<()> {
    // Physical pixels everywhere: UIA rectangles, window rectangles, captures and SendInput
    // then agree on every monitor whatever its scaling.
    unsafe {
        use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let desktop = Desktop::start();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async move {
        let service = WindowsUse { desktop }.serve(stdio()).await?;
        service.waiting().await?;
        Ok(())
    })
}

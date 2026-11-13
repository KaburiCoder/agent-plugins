# windows-use

Windows 데스크톱 프로그램을 보고·읽고·조작하는 MCP 서버(stdio)와 스킬. Windows 10/11 전용.

- **UI Automation 우선**: 요소를 텍스트로 읽고, 마우스 없이 값 넣기·누르기
- **Win32 컨트롤 트리**: UIA가 막혔거나 빈약한 앱, 다른 가상 데스크톱 창, WinForms 컨트롤 `Name`
- **스크린샷·이미지 찾기**: 가려진 창도 창 자체 그림으로 찍고, 크기가 달라도 그림을 찾음
- **입력**: 한/영 상태와 상관없는 글자 입력, 키 조합, 마우스 클릭·스크롤·드래그

## 설치

### Claude Code (서버 + 스킬)

```
/plugin marketplace add KaburiCoder/agent-plugins
/plugin install windows-use@agent-plugins
```

### 다른 MCP 클라이언트

Node.js 18 이상이 필요하다. 서버 실행 명령은 어디서나 같다.

```
cmd /c npx -y windows-use-mcp
```

Claude Desktop·Cursor·Gemini CLI 등 `mcpServers` 형식:

```json
{
  "mcpServers": {
    "windows-use": {
      "command": "cmd",
      "args": ["/c", "npx", "-y", "windows-use-mcp"]
    }
  }
}
```

VS Code(`.vscode/mcp.json`)는 키 이름이 `servers`이고 `"type": "stdio"`를 붙인다.

명령으로 추가하는 클라이언트:

```
claude mcp add --scope user windows-use -- cmd /c npx -y windows-use-mcp
codex mcp add windows-use -- cmd /c npx -y windows-use-mcp
```

Node.js 없이 쓰려면 [Releases](https://github.com/KaburiCoder/agent-plugins/releases)에서 `windows-use.exe`를 받아 그 경로를 등록한다.

```
claude mcp add --scope user windows-use -- C:/tools/windows-use.exe
```

스킬(Agent Skills 형식)을 지원하는 클라이언트는 [`skills/windows-use`](skills/windows-use) 폴더를 그 클라이언트의 스킬 폴더에 복사하면 사용 요령까지 쓴다. 스킬이 없어도 서버가 기본 사용법을 알려 준다.

## 도구

| 도구 | 하는 일 |
|---|---|
| `screen_info` | 모니터 배치·배율, 마우스 위치, 앞 창, 서버 권한 |
| `list_windows` | 최상위 창 목록 (process·title 필터, `wait_ms`) |
| `window` | focus · minimize · maximize · restore · close · move |
| `launch` | 프로그램 실행·문서/URL 열기, 새 창 hwnd 반환 |
| `screenshot` | 창·영역·모니터·요소·전체, 좌표 변환식 포함, `save_path` |
| `ui_tree` | 요소 트리 (`backend: uia/win32`) |
| `ui_find` | 조건으로 요소 찾기 (`wait_ms`) |
| `ui_at` | 화면 좌표의 요소 |
| `ui_read` | 요소의 값·글자 |
| `ui_set_value` | 값 넣기 (직접 설정 → 타이핑), 넣은 뒤 다시 읽어 확인 |
| `ui_click` | 마우스 없이 누르기 → 거부되면 마우스 |
| `mouse` | 좌표 클릭·이동·스크롤·드래그 |
| `type_text` | 글자 입력 (한/영 상태 무관) |
| `press_keys` | 키 조합 차례로 |
| `find_image` | PNG 템플릿 찾기 (`wait_ms`) |
| `clipboard` | 글자 읽기·쓰기 |

## 주의

- 사용자의 **실제 PC**를 조작한다. 포커스를 가져가거나 마우스를 움직이면 사용자의 작업을 방해할 수 있다.
- 관리자 권한으로 실행된 프로그램을 다루려면 서버(=MCP 클라이언트)도 관리자 권한이어야 한다.

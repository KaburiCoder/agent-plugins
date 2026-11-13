# agent-plugins

KaburiCoder가 만든 AI 에이전트용 플러그인과 MCP 서버 모음. 플러그인마다 폴더 하나다.

| 플러그인 | 하는 일 |
|---|---|
| [windows-use](plugins/windows-use) | Windows 데스크톱 프로그램을 보고·읽고·조작 (UI Automation·Win32·스크린샷·입력) |

## 설치

**Claude Code** (MCP 서버와 스킬을 한 번에)

```
/plugin marketplace add KaburiCoder/agent-plugins
/plugin install windows-use@kaburicoder
```

**다른 MCP 클라이언트**: 플러그인 폴더의 README에 클라이언트별 설정이 있다.

## 저장소 구조

```
.claude-plugin/marketplace.json   Claude Code 마켓플레이스 목록
plugins/<이름>/                    플러그인 (plugin.json, .mcp.json, skills/)
servers/<이름>/                    MCP 서버 소스 (Rust, cargo workspace)
npm/<이름>-mcp/                    npm 패키지 (서버 exe를 npx로 실행)
scripts/version.mjs               플러그인 버전을 모든 파일에 맞춤
.github/workflows/                태그를 올리면 빌드·배포
```

## 배포

1. `node scripts/version.mjs windows-use --set 0.2.0` (Cargo.toml·plugin.json·package.json을 같이 바꿈. 플러그인은 plugin.json 버전의 npm 패키지를 받는다)
2. 커밋하고 `git tag windows-use-v0.2.0` → `git push --follow-tags`
3. GitHub Actions가 테스트 → 빌드 → npm 배포 → GitHub Release를 만든다.
4. 사용자는 `/plugin marketplace update kaburicoder`로 새 버전을 받는다.

처음 한 번: npm 토큰(Automation 또는 publish 권한의 Granular token)을 저장소 Secrets의 `NPM_TOKEN`에 넣는다.

## 개발

저장소 폴더를 마켓플레이스로 등록하면 플러그인이 이 폴더를 그대로 읽고, 서버는 로컬 빌드(`target/release` → `target/debug`)를 쓴다. npm에 올리지 않아도 고친 것을 바로 시험할 수 있다.

```
/plugin marketplace add D:/mcp/agent-plugins
/plugin install windows-use@kaburicoder
```

- 스킬을 고쳤으면 `/reload-plugins`
- 서버를 고쳤으면 `cargo build --release` 후 `/mcp`에서 windows-use 다시 연결 (실행 중인 서버는 `%TEMP%` 사본이라 빌드가 막히지 않는다)
- 다른 exe를 쓰려면 환경변수 `WINDOWS_USE_EXE`

```powershell
cargo test
claude plugin validate .
```

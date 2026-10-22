# 04 Herdr 适配

文件：`gear/herdr/client.py`、`gear/herdr/profiles.py`。预算 220–280 行。
守住的不变量：观察失败不当没事；结果不明不当成功。

## 职责

- 一处封装所有 Herdr 调用：通过 `HERDR_BIN_PATH`（缺省 `herdr`）以 subprocess 调 CLI，带超时、解析 JSON、把错误分类。
- 显式绑定 session：引擎调用都带项目绑定的 session（`--session <name>`），不依赖 UI 焦点。
- 维护各 CLI 的 profile：读屏特征、resume 方式、能力声明。
- 分两层：`HerdrReadOnly`（命令行也可用，只读）和 `HerdrActor`（只有引擎用，有副作用）。

## 不做

- 不判断业务；不重试有副作用的调用（交给 08 ops 按"结果不明"处理）；不做事件订阅（MVP）。

## 对外接口

```python
class HerdrResult(NamedTuple):
    ok: bool
    data: dict | None
    error: str | None        # Herdr 的 JSON 错误码，如 agent_blocked / agent_prompt_stalled / timeout
    uncertain: bool          # 超时/进程被杀：动作可能已生效

class HerdrReadOnly:
    def __init__(self, session: str, timeout_s: float = 8): ...
    def agent_list(self) -> HerdrResult                     # 全 session 所有 Agent
    def agent_get(self, target: str) -> HerdrResult
    def agent_read(self, target: str, lines: int = 16) -> HerdrResult   # --source recent-unwrapped
    def pane_get(self, pane_id: str) -> HerdrResult

class HerdrActor(HerdrReadOnly):
    def prompt(self, target: str, text: str) -> HerdrResult          # 不带 --wait；开始与否由下一轮观测确认
    def pane_run(self, pane_id: str, command: str) -> HerdrResult   # 接入前在 shell 设置 GEAR_CTX
    def agent_start(self, name: str, kind: str, pane_id: str, args: list[str]) -> HerdrResult
    def send_keys(self, target: str, *keys: str) -> HerdrResult      # 用于让 Agent 退出（profile 指定）
    def pane_close(self, pane_id: str) -> HerdrResult
    def pane_title(self, pane_id: str, title: str) -> HerdrResult    # report-metadata --source gear --title
    def notify(self, title: str, body: str) -> HerdrResult
```

所有调用：超时默认 8 秒（`agent_start` 60 秒）；并发上限 4（全引擎共享信号量）；一个项目的慢调用不阻塞其他项目（08 按项目轮转）。

## CLI profile（`profiles.py` + 项目配置可覆盖）

```python
@dataclass
class Profile:
    kind: str                          # opencode / claude / codex / pi …（Herdr kind）
    busy_markers: list[str]            # 屏幕尾部出现即视为仍在跑，例 opencode: "esc interrupt"
    ratelimit_markers: list[str]       # 例 opencode: "Error: Rate limit exceeded"
    quota_markers: list[str]           # 额度耗尽特征（探针确定）
    auth_markers: list[str]
    tail_lines: int = 16
    confirm_rounds: int = 2            # 连续 N 轮命中才算
    session_ref: bool = False          # Herdr 是否上报 agent_session
    resume_args: list[str] | None = None   # 例 claude: ["--resume", "{session}"]；None = 不支持
    exit_keys: list[str] = ["ctrl+c"]  # 让 Agent 退出的按键序列（关会话前用）
    verified: bool = False             # 未经探针验证的 profile 不允许接管真实项目
```

- 内置 profile 只给**探针验证过的** CLI；其余需在项目配置显式提供并标 `verified=false` 时拒绝启用。
- 读屏特征只在尾部窗口匹配；催办模板在加载时校验不包含任何 marker（防"催办词触发自身检测"）。
- 命中逻辑（纯函数，便于测试）：

```python
def classify_screen(profile: Profile, tail: str) -> set[str]   # {"busy","ratelimit","quota","auth"}
```

## 结果分类

| Herdr 返回 | 含义 | 上层处理 |
|---|---|---|
| ok | 调用成功 | 对 prompt：只代表已写入终端（操作 SUBMITTED），是否开始由关联证据确认 |
| `agent_blocked` | Agent 在审批/提问界面 | 不输入；座位标需人处理 |
| `agent_prompt_stalled` / timeout / 被杀 | `uncertain=True` | 操作标 UNKNOWN，不重发，下一轮核对 |
| 已知未执行的明确拒绝 | 明确失败 | 操作标 FAILED，记录原因 |
| 未知错误、回执不可解析、连接中途断开 | `uncertain=True` | 操作标 UNKNOWN，不重发 |
| 连不上 Herdr | 观察失败 | 本轮所有判断跳过，看板显示"观察失败" |

## 依赖的探针（15）

各 CLI：busy/限流/额度/认证特征、工具运行中是否误报 idle、`agent_session` 是否上报、resume 参数与成功判定、退出按键。`pane run` 在 shell 中 export 后 `agent start` 的环境继承。

## 测试要点

替身 subprocess：超时→uncertain；JSON 错误码映射；classify_screen 对 marker 与催办词的回归样例；并发上限。

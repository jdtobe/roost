# Adding an agent tool

Every built-in agent tool is one `impl Tool` in `crates/roost-core/src/agent/tool/`. A tool decides what a call looks like and how it runs; the registry (`agent/registry.rs`) decides whether it may run at all (permission, Settings, the session's switches). A tool never checks permission itself.

## 1. Create the file

Add `crates/roost-core/src/agent/tool/<name>.rs` with a unit struct and an `impl Tool for ...`, and declare it in `tool/mod.rs` next to `mod ask; mod files_read; ...`. A tool with no logic of its own can call a function from `agent/tools.rs` (see `files_read.rs`). Existing files are the best templates: `files_read.rs` (plain, with options), `files_write.rs` (previews and a recorded change), `shell.rs` (commands), `ask.rs` (a question), `web.rs` (the network).

## 2. The `Tool` trait

All in `tool/mod.rs`:

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn spec(&self) -> ToolSpec;
    fn needs_workspace(&self) -> bool;
    fn exec(&self) -> Exec;
    fn options(&self) -> &'static [OptionDecl];
    fn plan(&self, cx: &PlanCx, args: &Value) -> Plan;
    fn run(&self, cx: &RunCx, args: &Value) -> (Result<String, ToolError>, Option<change::Change>);
}
```

- `name`: what the model calls it; must equal `spec().name`.
- `spec`: a `ToolSpec` (`registry.rs`) with `name`, `family`, `capability`, `description` and `parameters` (a JSON schema object). The description and schema are sent to the model word for word.
- `needs_workspace`: true if it cannot run without a folder. A Work session with no folder pauses on the "No working directory" card for such a call.
- `exec`: see below.
- `options`: limits the user may lower per session (see below), or `&[]`.
- `plan`: looks at the arguments before anything runs and returns a `Plan`.
- `run`: does the work. Returns the text the model reads, or a `ToolError` that the model reads instead; the second value is the `Change` of a write (what Undo uses), `None` for everything else.

`PlanCx` has `workspace: Option<&Workspace>`, `limits` and `questions_asked`. `RunCx` has `workspace: &Workspace`, `limits` and `opts: &Options` (the call's approval, undo room and cancel flag, not option values). The session's options are already applied in `cx.limits`, so read limits from there, as the existing tools do.

### `Plan`: what the chat loop does before running

- `Plan::Run`: run without asking, even under Ask. Use it for a call that cannot be planned (bad arguments): it changes nothing, and running it lets the model read why it failed.
- `Plan::Approve { scope, preview, run }`: under Ask, show the approval card and wait; under Auto, run; under Off, nothing runs. `preview` is the diff of a write, `run` the command of a run. For a tool with nothing to show, return `tool::by_capability()`.
- `Plan::Question`: put a question to the user and use the answer as the result (`ask_user`).
- `Plan::Refuse(ToolError)`: refuse without running.

### `Scope`: what "Allow for this chat" remembers

- `Scope::Capability`: the whole capability becomes Auto (what `by_capability()` gives). Only read tools (and asking) may plan it: `max_permission` is Auto for every capability, so for a tool that writes, runs commands or uses the network, "Allow for this chat" would raise the whole capability. Those tools use a narrower scope. A registry test (`only_read_tools_and_asking_plan_a_capability_wide_allowance`) enforces this.
- `Scope::File(path)`: one workspace-relative file.
- `Scope::CommandPrefix(prefix)`: plain commands starting with that prefix.
- `Scope::Once`: nothing is remembered; only this call.
- `Scope::Host(host)`: one lowercased web host.

Pick the narrowest scope that is still useful. A new kind of scope means a new variant here and in the code that stores allowances.

### `Exec`: where a call runs

- `Exec::Blocking`: on the blocking pool, through `run`. Almost every tool.
- `Exec::Network`: awaited on the network by the chat loop; `run` is not used (`web.rs` returns an error from it). The loop (`chat/agent_loop.rs`: `start_tool_call`, `fetch_for_agent`) only knows URL fetches today, so a different network tool needs work there.

### `OptionDecl`: limits the user can lower for one session

```rust
pub struct OptionDecl {
    pub key: &'static str,   // unique within the tool, e.g. "limit"
    pub label: &'static str, // shown in the tool list
    pub help: &'static str,
    pub min: u64,            // the floor; the ceiling is `get` on the Settings limits
    pub get: fn(&Limits) -> u64,
    pub set: fn(&mut Limits, u64),
}
```

`get` and `set` read and write one field of `Limits` (`agent/tools.rs`), so the field must exist there (it is also a `[tools]` key in `roost.toml`). An option can only lower a value, never go above the Settings one. A tool with no limits returns `&[]`.

## 3. Register it

Add `&<file>::<Struct>` to the slice in `all()` in `tool/mod.rs`. The order there is the order the model is told about the tools and the order of the tool list, so a new tool goes where it should be read, usually at the end of its family (the list groups by family but keeps this order). `registry::specs()` and `tool::find(name)` are built from `all()`.

`family` and `capability` in the spec place the tool in the permission model. Reuse an existing pair (Files/Read, Files/Write, Shell/Run, Ask/Ask, Web/Fetch) when it fits. A new `Family` or `Capability` also needs arms in `registry.rs` (`label`, `capability_key`, `default_permission`, `max_permission`) and a place in the GUI's permission pills; treat that as a separate change.

## 4. The golden file

`crates/roost-core/tests/golden/tool_definitions.json` holds the exact tool definitions sent to Ollama with every tool on. The test `the_tool_definitions_sent_to_the_model_have_not_changed` (in `registry.rs`) fails when they differ. For a new tool, regenerate it and read the diff:

```
UPDATE_GOLDEN=1 cargo test -p roost-core the_tool_definitions_sent_to_the_model_have_not_changed
git diff crates/roost-core/tests/golden/tool_definitions.json
```

Only your tool's entry should be added. A changed description or schema of an existing tool changes what the model sees, which is a behaviour change: do it on purpose, say so in the commit, and re-check with a real model.

## 5. Tests a new tool needs

In the `tests` module of `agent/registry.rs`, plus the tool's own file:

- Golden: regenerated as above.
- Shape: `every_registered_tool_has_a_unique_name_a_description_and_an_object_schema` covers name, description and schema for every tool in `all()`; make sure it passes, and add a test of the arguments you accept and refuse.
- Approval path: a test that the call asks under Ask, runs under Auto and does not run under Off (see `a_write_tool_changes_the_file_only_once_approved` and `an_approved_call_may_run_a_tool_that_asks_but_never_one_that_is_off`), and that `plan()` returns the `Scope` you chose, including for bad arguments.
- Options: if the tool has `OptionDecl`s, `every_option_changes_its_own_limit_and_nothing_else` iterates every declared option and asserts how many there are, so raise that count for yours; check the floor in `min` too.
- `run` itself: a test for its result and for its errors, with a `tempfile` workspace.

Then run the gate: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings` (also with `--features dev-hooks`) and `cargo test --workspace`.

## 6. The GUI

A plain tool needs nothing: it appears in the tools popover (Individual tools list) and in the "All tools" window, under its family, with its switch, status and options, all built from `tool::all()`.

Every registered tool needs an entry in `summary` and `facts` in `crates/roost-gui/src/chat/tool_limits.rs` (the short phrase beside the tool and the full set in the limits popover); the test `every_registered_tool_has_a_summary_and_limits` fails without one. A tool with no limits of its own gets a short line saying so, like `ask_user`. Add a line to `CHANGELOG.md` under the Tools section.

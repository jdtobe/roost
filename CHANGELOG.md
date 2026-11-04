# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project intends to adhere to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it has a first release. There are no releases yet.

## [Unreleased]

Pre-release. The core library and the desktop app (Browse, model card, downloads dock, Installed, Settings) are implemented and tested; nothing is published yet. The macOS release is packaged as a universal app inside a DMG.

### Roost foundation

- The project is renamed Roost: crates `roost-core` and `roost-gui`, binary `roost`, settings `roost.toml`, keychain service `roost`, environment variables `ROOST_*` (the `HF2O_*` names are no longer read).
- One-way, non-destructive migration of hf2ollama settings and import history; legacy staging markers are still recognized; the Hugging Face token is imported only when the user asks in Settings.
- Metadata writes (settings, history) are atomic and collision-safe.
- Four theme families (Meadow, Ember, Plum, Tide), each light and dark, chosen in Settings; the old `theme` setting still loads. Palettes meet 4.5:1 text contrast and are layout-tested at the minimum window size.
- The Roost mark and icon, generated from `docs/design/roost-tail.svg`, and a CI scan for stray old names.
- Serializable chat and agent session types in `roost-core`. Chat and agents themselves are not implemented yet.

### Work without a folder

- New Work opens a session at once with no model and no folder (the New agent form is gone). The model popup marks which models support tools, asking Ollama about the ones it does not know yet, and sending says "Choose a model first." until one is chosen.
- A Work session can start with no working directory. When a file or command tool needs one, the run pauses on a card ("No working directory is attached") with Choose folder… and Skip; choosing runs the paused call (through approval as usual), skipping fails it so the model carries on. The folder pill shows "No folder" and does the same. Reopening the chat asks again, and a switched-off tool fails without asking.
- Work sessions start with tools and the folder locked. The tools button, the folder pill, the R/W/N pills and `Working folder…` stay off until the chosen model is confirmed to support tools (Roost asks Ollama as soon as a model is chosen). A model without tools turns the tools button red and says the session is effectively chat only. The "Choose a model" chip is red while no model is set and flashes inverted when you press Enter without one.

### Chat and history (stage 2)

- A Chat view, now the first tab and the starting view: streaming replies from any installed Ollama model, a history rail (Pinned, Today, Yesterday, Earlier), New chat and New incognito, rename, pin and two-step delete, a model picker, Retry for failed, stopped or interrupted replies, and a Stop button while a reply runs.
- Conversations are saved one file per session with a rebuildable index. Damaged files are set aside, files from a newer Roost are left untouched, and a crash during a reply keeps the partial text (marked Interrupted). Only one Roost can use a history at a time.
- Incognito chats exist only in memory: nothing is written to disk, and their drafts and editor state are dropped when they end.
- Requests send an explicit `num_ctx` (the model's context length, capped at 16,384) and trim old exchanges to fit, keeping the full transcript locally.
- Replies continue in other sessions and while you use Browse, Installed or Settings; the Chat tab shows a count of running replies.
- Fixed: the top bar's logo, tabs, search field and Ollama status pill now share one vertical centre.

### Chat refinements

- A delete control on every rail row: hover or select a row, click the ✕, then click "Delete?" after a short pause.
- Loaded models: the chat header shows whether its model is in memory (and when Ollama will unload it) with an Unload button, and the rail has an "In memory" list with Unload for each loaded model. Ollama is asked every 10 seconds and whenever a reply starts or ends.

- Chats without a title show the start of their first message in the rail and the header (display only; a rename always wins).

### Agents can write files (first step)

- New tools `write_file` and `edit_file` (replace one exact piece of text). Both stay inside the agent's folder, refuse protected names (`.env`, keys and the like), and ask for your approval by default: writing is a new capability that is **Ask** unless you set it to Allow (or Off) for an agent.
- The approval question comes with a preview of the change: the path, whether the file is new, lines added and removed, and the changed lines with context. The approval card in the window shows it: what the agent wants to do, the file, `+n −n`, the changed lines in a small scrolling box, and "… and n more changed lines" when there are many.
- "Allow for this session" on a write allows edits to that file only, for as long as the chat is open, and never turns writing on for everything.
- The header shows an **R** and a **W** chip beside Agent settings: accent when allowed, amber when it asks first, dimmed when off, with a tooltip saying exactly what each allows. On a narrow window the Agent settings button is the gear alone, so the title and folder keep their room.
- The Tools card in Agent settings has a Read row and a Write row (each Off / Ask / Allow), then "Individual tools · n of m on".
- Settings has a new **Agents** card with "Let agents write files". Off makes every agent read-only: the write tools are not offered, a forced call is refused, and the W chip and Write row disappear. Older settings files keep writing on.
- The permission labels read Off / Ask / Allow instead of Auto.
- An agent's folder is now its own line under the title, without parentheses, so it has the whole column to itself; it is still cut in the middle when it is too long and the full path is in the tooltip.
- **Every write can be undone.** Before a change, the original is kept in the chat's own file (not in your project), up to 8 MB per chat; a write that would not fit is refused before anything is touched. Under the tool call a row says what changed (`Changed src/config.rs +3 −1`) with **Show diff** and **Undo this change**; Agent settings has a **Changes made** card with **Undo all** (newest first). Undo refuses, and says why, when the file was edited after the agent wrote it, when the agent is working, or when the agent has since moved to another folder; it never overwrites your own edits. A new file is removed on undo.
- Because the originals are kept in the chat file, deleting the chat deletes them. A branch starts with no changes of its own and cannot undo the original's.
- Agent prompt no longer says "read-only".

### Agents can run commands (opt-in)

- **Off until you turn it on.** Settings → Agents has **Let agents run commands**, off by default. Turning it on shows a warning first (a command runs as you, is not limited to the agent's folder, can use the network, and a mistaken or manipulated model can do damage Roost cannot undo); **Keep it off** is the default button and Escape also declines. Until it is on there is no X chip, no Run row and no Shell card anywhere, and the chat core refuses commands even for an agent that was set to Allow.
- **A `run_command` tool** (the Shell family) runs a command in the agent's folder: it stops after 2 minutes, keeps the first 20 KB of output, has no input, and does not pass on your environment's secrets (variables starting with `HF_`, `ROOST_`, `AWS_`, `GITHUB_`, `GH_`, `ANTHROPIC_`, `OPENAI_` or `SSH_`, or whose names contain TOKEN, SECRET, PASSWORD, CREDENTIAL, API_KEY or PRIVATE_KEY). Stop ends the command and everything it started.
- **The approval card** shows the exact command, the folder, the time and output limits and that your tokens are not passed on. **Allow `cargo test` this session** covers that command and its plain subcommand only; it is left out for anything chained, a path, a shell or an interpreter, `rm`, `curl`, `ssh`, `sudo` and similar. A very long command scrolls in its box.
- **An X chip** joins R and W in the header (red when allowed, amber when it asks, dim when off). **Run commands** is a row of Off / Ask / Allow in Agent settings; choosing Allow asks for one more confirmation.
- **Tools by family.** Agent settings now has one card per family: **Files** (Read and Write rows) and **Shell** (Run row), each with its tools underneath, tagged R, W or X and switchable one by one. The lists start closed. The Agent settings menu scrolls when it is taller than the window.
- **`read_binary`** reads any file as a hex dump (offset and length in decimal or `0x` hex, up to 4096 bytes a call, with the next offset to continue from), so an agent can look inside ROMs and other non-text files. `read_file` now points to it when it meets a binary file.

### Tool limits and byte search

- **Every tool limit can be set** in `roost.toml` under `[tools]`; any key may be left out. Keys: `list_entries`, `read_default_lines`, `read_max_lines`, `output_bytes`, `find_results`, `depth`, `examined`, `search_matches`, `per_file_matches`, `line_chars`, `read_file_bytes`, `search_file_bytes`, `bytes_file_bytes`, `write_bytes`, `edit_file_bytes`, `undo_bytes`, `run_timeout_secs`, `run_output_bytes`. A value out of range is moved to the nearest end instead of breaking the tools (for example nothing can be zero). Changes reach running agents when settings are applied. The sizes for writes, edits and undo copies, which were fixed, are now among them.
- **A cut result says exactly what was left out.** `list_dir`, `find_files` and `search_text` take an `offset` and end with, for example, `[showing matches 1-200; there are more. Call search_text again with offset=200 for the next]`. `search_text` also says when a file had more matching lines than the per-file limit shows (searching that one file on its own is not held to it) and which files it could not look inside and why (binary, too large, not UTF-8 text), with a hint to use `read_binary`.
- **Settings → Tools** has the limits people are likely to change: entries per folder listing, results per file search, matches per text search, lines per file read, output per tool call, size of one file write, undo copies per chat, the time a command may run and the output kept from it, each a pick from round values (a value set by hand in `roost.toml` is shown too), and **Reset tool limits**. Changing one does not restart the download engine.
- **The model can ask for what it needs.** `list_dir`, `find_files`, `search_text` and `search_bytes` take a `limit` (fewer results per call; never more than the Settings ceiling, and it says so when it was lowered). `list_dir` and `find_files` take `sort` (`name`, `size` for largest first, `modified` for newest first) and `reverse`, and show the size or time beside each name, so "the largest ROMs" is one call. Paging follows the chosen order.
- **Results fit the model.** One tool result may take about a sixth of the model's context window, so a model with a small window gets smaller results (the Settings value is the ceiling) instead of a run that fails for lack of room. The command approval card shows the output size that will really be kept.
- **`search_bytes`** finds a byte pattern in a file or across a folder, for example a ROM header: hex pairs with `??` for any byte (`4E 45 53 1A`, `de ?? be ef`), an optional `glob` such as `*.nes`, and results as `path:0xOFFSET: bytes` with the same paging and limits. Files are read in pieces, so a large file is never held in memory, and a pattern that straddles two pieces is still found.

### Message times and notifications

- Each message row shows when it was sent, muted, after its icons (`14:03` today, `Oct 28, 14:03` earlier, with the year for earlier years). The bubble's tooltip has the full date and time with the weekday.
- The "Used N tools" line of an agent run also says when the first call started; its tooltip has the full date and time. Each tool card shows its own start time.
- Notifications for agents: when an approval is waiting or a run finished or stopped, Roost bounces the Dock icon (flashes the taskbar) and sends a system notification while its window is not in front. Settings, Agents: when Roost is in the background (default), always, or off. Only the first alert of a visit notifies; the window title counts the rest, for example `(3) Roost`. The text is a fixed sentence and never includes file contents, commands or model output. A run you cancel does not notify.

### Tool limits in the Work header

- In an agent's Individual tools list, each tool shows how much it can return (for example `list_dir  200 entries`). Clicking it shows all of that tool's limits, with a button that opens Settings, Tools, where they are changed.

### Agents can ask you questions

- "All tools…" in the agent's tools menu opens a window that lists every tool by family: what it does, whether it is off, asks or is allowed in this chat, an on/off switch, its limits, and what the model is told about it.
- A tool's limits in the agent's tools list can now be lowered for just that chat: open a tool's limits and drag or type a smaller value (never above the one in Settings), or press Reset to go back to the Settings value.
- A new tool, `ask_user`, lets an agent stop and ask: pick one of 2 to 8 options, pick several, or type an answer. Pick questions also take an answer of your own, and you can skip. The question appears above the input, the run waits (also while you look at another chat), and your answer goes back to the model. The question and your answer stay in the chat.
- It is on by default and can be switched off in the Individual tools list ("Ask you"). An agent may ask up to 5 questions per run. A question counts as an alert while the window is in the background ("Has a question for you").

### A compact input

- The input at the bottom is one rounded bubble that starts a line high and grows with the text (up to a limit, then it scrolls). The send button is a round arrow inside it, at the bottom right; while a run is going it becomes a red Stop.
- Small pills sit under it. On the left: **+** (an agent's menu offers the working folder; files and images are not supported yet), a hammer for the tools and settings popover (it opens above the button), and the agent's folder. On the right: the **R**, **W** and **X** pills and the model.
- The R, W and X pills moved from the header and are now buttons. A click steps Off, Ask, Allow; running commands steps only Off and Ask, and **holding the X pill** asks (once, above the input) before allowing commands without asking. The pills are locked while a run is active.
- Nothing sits behind the input: no lighter panel or border, only the bubble, on the same background as the conversation.
- In a narrow window the folder shrinks to its icon and the model name is shortened. The header keeps the title, the folder path, the status, the pin and the trash can; the "Agent settings" button is gone from it.

### A simpler top of the sidebar

- The "New chat" button with its arrow is replaced by two icon buttons beside the search field: new chat and new incognito chat. On the Work tab they start a new agent or a new incognito agent. The tooltips show the keyboard shortcuts.

### Deleting a chat can be undone

- Deleting a chat still takes a click and a confirming click (on the sidebar row's trash can and on the header's), and then a small "Deleted … Undo" notice stays for 10 seconds; Undo brings the chat back as it was. The sidebar's x is now a gray trash can (red on hover). A second click inside the first 0.1 seconds is ignored, so a quick double-click can never delete (this pause was 0.6 seconds, long enough to make the confirm look ready when it was not; it is now 0.1 s for chats, installed models and Clear cached files).
- A deleted chat waits in a `trash` folder inside the history folder, and is removed for good when the notice goes away, when you delete another chat, or at the next start. Incognito chats have nothing on disk and cannot be brought back.

### A richer history list

- Each row in the sidebar now shows the start of the last message (yours or the reply) instead of the model name, which was the same on every row, and the time at the right: the clock for today, the date for earlier. A chat with nothing in it reads "New chat" in grey italics with "No messages yet". An incognito row has an eye icon, a thin edge on the left and "Incognito · not saved". A pinned row has a pin icon.
- On the Work tab the second line is the agent's folder (with `~`, cut in the middle when it is long), and a badge at the right says **Working** while a run is going and **Needs you** (amber) when an approval or a question waits.
- The saved list (`index-v1.json`) gains the last message and the folder; an older index is refreshed the first time it is opened.

### Why an agent's run stopped

- When an agent's run ends early, the chat now says why at the end of the transcript, and the reason is saved with the chat, so it is still there after you leave the chat or restart Roost. The four endings: it used all its steps (with a Steps box, filled with 30 more than before, and **Continue**), you stopped it (**Continue**), a model turn failed with the reason (**Try again**), and Roost was closed while it was working (**Continue**). The text takes the width; the controls sit below it, at the bottom right. Continue sends a plain "Continue" message, so the chat shows what happened.
- Once the conversation moves on, the banner becomes a quiet one-line note where the run ended and its buttons go. A run that finishes normally clears the note. Chats do not get one (their failed replies already have Retry).
- Saved sessions gain an optional `last_run_end` field; older files load as before.

### Models that write a tool call as text

- Some models (a small `qwen2.5-coder`, for one) answer an agent with the JSON of a tool call, such as `{"name": "list_dir", "arguments": {}}`, instead of making a real call, so nothing runs. Roost used to show that JSON as the agent's answer. When a finished reply is only calls to Roost's tools written as JSON (one, several, in an array or in a code fence), the chat now says "The reply is a tool call written as text. Nothing ran (list_dir). This model may not handle tools well here; try another model." There is no Continue button, since going on would not help. An ordinary answer that mentions such JSON in a sentence is not affected.
- The run counts as stopped for notifications, not as finished.

### A clearer warning for running commands

- In the "Let agents run commands?" warning, the "Keep it off" button was filled with the theme colour, which is also the colour of an ON switch, so it read like "on". It is now a neutral button (still the default one, so Enter never turns commands on), and "Turn on" keeps its red outline.

### Flatter edges

- The sidebar (Chat, Work and Models) no longer has a border on all four sides, and the top bar, the status bar, the downloads dock and the menu bar no longer outline themselves. Each bar keeps the single line the panel draws along the edge that faces the content, and the sidebar keeps one divider on its right, so the top and bottom edges no longer double up with the bars above and below.

### A top bar with a little colour

- The top bar has a 4 px strip of the theme colour along its top edge, and the tabs are flat: no grey track and no pill; the active tab is bold in the theme colour with an underline of it, and the others are grey. Settings (the gear) is not a tab, so nothing is underlined there.

### A tinted sidebar

- The sidebar (Chat, Work and Models) takes colour from the theme: the list is a half-strength tint of the theme's light colour, the search field and buttons (and, on Models, the filters) sit on a band of the full tint, the selected row is white with a dark 3 px edge on its left, and the divider on the sidebar's right is a darker tint. The tints are worked out from each theme's own colours, so all four families and both modes get a matching sidebar.

### A calmer Work and Chat page

- The page header is a deeper tint of the theme colour (one family in every theme, so the pink-against-purple clash is gone), and it is the same height (60 px) as the sidebar's lighter search band beside it. The title is a little smaller, the folder under it is set in a few pixels, and the buttons and title are centred in the band, close to its sides (10 px). The tile with the chat or agent glyph in the header is bigger than the buttons beside it (44 px, with the glyph scaled up and a darker edge). The Models and Browse headers use the same deeper tint. Muted text on it is a touch darker to stay readable.
- The sidebar list is a lighter tint, and the strip at the top of the window is 3 px.
- The two buttons beside the search field are both outlined, with the new-chat plus in the theme colour; either one gets a faint theme-colour outline when you point at it.
- The session tile in the header is neutral. The R, W and X permission pills stay one colour and change shape instead: solid means allowed, outlined means asks first, dashed means off. Running commands, when allowed, is solid red.
- A reply that only thought, with no words, is one quiet "Thinking" line instead of an empty card, and it no longer reserves the empty row of icons beneath it.
- The downloads dock is out of the way when nothing is running: it slides down and fades a few seconds after the last job is dismissed. A downloads button at the right of the status bar (with the number of running jobs) opens it by hand and folds it away again, even while jobs are running; a new job brings it back.
- The "Start a chat" and "Start an agent" text shown when nothing is open is centred in the pane, a little above the middle, instead of sitting in the top-left corner.

### A calmer Models tab

- The top of the Models list is one 60 px band like Chat's: the search field with a sort button and a filter button beside it. Sort has its own popover with the four orders.
- The disk usage display is two flat lines with a thinner two-tone bar instead of a bordered card, and it sits on a white strip above the Installed heading.
- Installed models are two lines: the name with its size at the right, then where it came from, its parameters and its quantisation (`unsloth · 27.3B · Q4_K_M`). The monospace names and the pills are gone.
- An installed model's page has the same header as a chat: a 44 px tile (with a small neural-net icon), the name and facts in the deeper tint, and Chat, Work and delete at the right. Work is outlined and delete is a quiet icon that turns red when pointed at. The details sit on the page background and About stays on white.

### A flatter Settings page with search

- Settings has a header band in the deeper tint, like the other tabs, with the page title, a search box and the save state ("Changes apply immediately", "Saving…", "Saved").
- The seven bordered cards are flat sections: a heading with a thin rule in the theme colour, then its rows with hairlines between them. A narrow column of heading links at the left jumps to a section and shows the one being read.
- The search box filters the rows by their label and help text. A section keeps its heading only while one of its rows matches, the links follow, and a search with no match says so.

### Same hover on the icon buttons

- The sort and filter buttons on the Models tab now get the same hover as the new-chat buttons in Chat and Work: a faint outline in the theme colour. The look is one shared function, so they stay the same. The search box on Models says just "Search", like Chat and Work. The two lines in each installed model's row are centred in the row, so the space above and below them is the same in every row (the second line used to sit close to the divider).

### Rows in the Models list no longer overlap

- Each row in the Installed and Hugging Face lists started partway down the one above it, so the dividers and the selected or pointed-at tint were drawn over the row's own text and the spacing looked uneven (the first row was fine and the ones below were not). Rows now follow each other exactly, and the two lines in an installed row are centred in it.

### Agents can read web pages

- A new tool, `fetch_url`, lets an agent fetch one web page and read its text. It is a fourth permission beside R, W and X: a globe pill under the input (off by default, drawn dashed), also in the agent's tools list under "Fetch web pages". The pill steps only between Off and Ask; letting every page through is chosen in the tools list.
- With Ask, a card shows the exact address and offers Fetch, "Allow <host> in this chat" (that host only, for this chat only; another host, even after a redirect, asks again) and Deny.
- It fetches over https only, with no user name or password in the address, 20 seconds at most, up to five redirects and a size cap, and returns readable text (no scripts or styles) that is marked as untrusted text from the web. Pages that are not text are refused.
- Addresses on this computer or the local network are always refused, whatever you answer: `localhost`, private and link-local numbers, names like `.local` or `.lan`, and a name that resolves to one. The check is repeated after every redirect, and the request goes to the address that was checked, so a page cannot steer an agent at Ollama or a router.
### A download time estimate that holds up

- The estimate under a download could say "~5 s left" for most of a long download. Progress reports were timed when the window handled them, so when the window was not being redrawn (in the background, say) minutes of progress arrived at once and looked like an enormous speed, and the estimate stayed tiny for a long time after. Reports that arrive together are now one report, a long silence is measured as a long silence, the estimate reads the speed over the last 20 seconds instead of a slow running average, and it never trusts a speed more than three times the average so far. While a download is running in the foreground it behaves as before.

### Quitting with work in progress

- Closing the window while a chat reply or an agent run is in progress now asks first, like it does for downloads, and says what is running. Quitting stops those runs and keeps what they had written so far.

### One sidebar

- Chat, Work and Models share one left column of the same width (340 by default, as wide as the Models list was). Drag its right edge to change it, between 280 and 520 points; a narrow window limits it so the page beside it keeps at least 540. The width you choose is remembered between runs and is the same on every tab. Saving it (and changing the theme or the auto-dismiss time) no longer restarts the download engine: only settings the engines use (the Ollama address, the staging folder, the delete/keep options, simultaneous downloads, the Hugging Face address and keep-alive) rebuild them. Before, any settings change did, which let a download started afterwards ignore the simultaneous-downloads limit.
- Search is the first thing in every left column. The Models search moved out of the header into the top of its column, above the filters; the Chat and Work search now sits above **+ New** and is always shown. Every search box has a clear (✕) button once it holds text.

### Chat and Work header

- The title row is a header band (the theme's header tone): a tile showing the kind of session (a speech bubble for a chat, a bubble with a spark for an agent), the title in a larger, bolder type (an untitled session reads "Untitled" in grey italics), and for an agent its folder in parentheses with the home directory written as `~`, cut in the middle when it does not fit and shown in full on hover. On the right: a status dot with Idle, Working (with the step) or Waiting for you; for an agent an **Agent settings** button with a gear; a pin that fills when the session is pinned (not offered for incognito chats) and a red trash can that asks twice, the second time saying "Click again to delete". A pencil beside the title (or a double-click on it) edits the name; Enter or clicking away keeps it, Escape cancels.
- Agent settings opens as three cards (Folder, Run, Tools); the Off / Ask / Auto choice is a segmented control with the chosen segment filled.
- A tool approval question now sits above the message box instead of at the end of the conversation, so it cannot scroll out of sight.
- Right-click the app name for a quick theme switcher: one dot per family and System / Light / Dark.

### Theme roles

- Every theme family gains a **second hue** (steel blue for Meadow, deep teal for Ember, rose for Plum, violet for Tide, each with a soft background and a darker/lighter pair for dark mode) and a **header** tone. Links and the "installed" tag use the second hue; the page headers on the Models tab use the header tone instead of the accent tint; your chat messages sit on the raised surface with an accent edge instead of a filled accent bubble. The accent keeps primary actions and selection. A test checks the text contrast of the new pairs in all eight palettes.

### One Models tab

- **Models** is one view instead of Browse and Installed behind a switch: a list with the models Ollama has (with a usage bar) above the Hugging Face results, one search box that narrows both, and one detail pane. Filters are chips above the list (a Filters popover, Fits my machine, and the sort order) instead of a side rail.
- Selecting an installed model shows **Chat**, **Work** and **Delete…** (two clicks, with the usual pause), then its details and where it came from; the source repository is a link that opens it in the same list.
- The list: installed rows are taller, with the parameter count, quantization and size as pills; the disk-usage bar is a titled card with a legend; the sort order is a full-width dropdown with a filter button at its right end (the button keeps its place and shows how many filters are on; its popover holds every filter, including Only models that fit, and a Reset button). Hugging Face results default to GGUF only.
- Model pages are three stacked bands in the theme's own colours. An installed model: the header with Chat, Work and a red trash can (tinted with the accent), the details, then the description. A Hugging Face repository: the header with any warnings, the files to choose from, then the description (no more Files/About tabs). The import panel at the bottom appears only after you pick a file; nothing is picked for you any more (the recommended file is still marked).
- A Hugging Face result for a repository you have imported from shows an **installed** tag. The installed model's detail ends with **About**: the source repository's description (or a note that models pulled from the Ollama library have none).
- The View menu has one Models entry (⌘3); ⌘4 and the separate Browse and Installed entries are gone.

### A quieter window

Five rounds of design (mockups and evaluations in `docs/design/ui-declutter-mockups.html`); the busiest screen went from about 25 visible controls to about 13.

- **Tabs:** Chat, Work and Models, plus a settings gear. Browse and Installed are the two views inside Models (a small switch next to the tabs). The menu shortcuts still reach each of them.
- **Title row:** the title, a folder chip for agents, and one “···” menu holding Rename, Pin and Delete (delete asks for a second click). Incognito is a small badge instead of a banner.
- **Folder chip (Work):** the folder, step budget and tools are one popover instead of a second bar. It locks while a run is active.
- **Composer:** the model chip sits under the input and shows whether the model is in memory; its popover changes the model and unloads it. One status line above the input shows an agent's step (“Step 3 of 20”), messages waiting for its next step, and notices.
- **Tool steps** of an agent fold into one line (“Used 3 tools: search_text ×2, read_file”) with live “running…” and failure counts; open it for the cards.
- **Sidebar:** one “+ New” menu replaces the two buttons; the “In memory” footer moved into the Ollama status pill's popover, which lists loaded models with Unload.

### Fixes and polish after first use

- **+ New** is a split button: the main half starts a chat (or agent), the arrow offers the incognito variant.
- Shift+Enter adds a line in the message box; Enter sends.
- On narrow windows the status pill shows only the Ollama version (the address is in its hover text), so the Browse search field keeps its room.
- The import plan card shows its steps as one line (`Download › Verify › Upload › Create`) that never wraps.
- A search field in the Chat and Work sidebars (matches the title, first message and model).
- Assistant replies are drawn as Markdown (bold, lists, code blocks, tables, safe links) and every message has a Copy button.
- **Message actions:** a row of icons under a message (always on the latest, on hover for older ones): Copy, Edit (your messages), Regenerate (latest finished reply) and Details (model, tokens in and out, speed and time, for replies finished from now on).
- **Versions:** Regenerate and Edit keep what they replace. A message with other versions shows `‹ 2 / 3 ›` at the bottom-right of its row; the arrows switch between them (when no reply is running). Edit opens your message in place and sends it as a new version. Saved chats store the other versions; older files load unchanged.
- **Branch:** the branch icon on a finished reply starts a new chat with the history up to that reply, on the same or another model. A branched chat shows `↳ Branched from …` at the top (each name opens that chat; long chains collapse) and says whether the original is unchanged, changed since, or deleted; a divider marks where the copied history ends. Branches are listed under their parent in the sidebar.
- **Side questions (BTW):** the speech-bubble icon on a reply opens a popover where you ask something about the conversation up to that reply. It is answered separately (it works while a reply or agent run is going), is never added to the chat unless you press Add to chat, and is remembered per chat (newest first, up to 50, kept in the chat file; incognito chats keep them in memory). Earlier ones are listed under the answer, each with Add to chat and Delete.
- **Read aloud:** the speaker icon on a finished reply reads it with the computer's own voice (it becomes a Stop button while reading). macOS uses `say`, Windows uses PowerShell's speech synthesizer, and Linux uses `spd-say`, `espeak-ng` or `espeak` when one is installed; otherwise the icon is greyed with the reason. Markdown marks and code blocks are not read. No speech library is added.
- Fixed: the **+ New agent…** button did nothing while an agent chat was open. The New agent form now replaces the chat on screen.
- **Tool approvals:** the agent chip's Tools section now sets Files · Read to Off, Ask or Auto. With Ask the model still sees the tools, but each call waits for you: a card in the chat shows the tool and its arguments with Allow, Allow for this session (switches the permission to Auto) and Deny (the model is told you declined). Stop ends the wait, and reopening the chat shows the question again.
- **Settings ▸ Ollama ▸ Keep models in memory:** Ollama's default, 15 minutes, 1 hour, or until Ollama quits. Sent as `keep_alive` with chat requests only when you choose something other than the default.
- **Downloads panel:** each job is two lines. The name (cut in the middle so the tag stays visible), size, a stage pill and the buttons; then a progress bar of the same width for every job with percent, bytes, speed and an estimate of the time left. The estimate uses a smoothed speed, waits for five seconds of data, is rounded, and does not jump about. The log drawer gained a Copy button.

### Agents (Work tab)

- A **Work** tab (between Chat and Browse) for read-only agents. An agent works inside a folder you choose and can list, find, search and read files there; it cannot change anything. Start one with New agent: pick a folder and a model that supports tools (Roost checks with Ollama first).
- The agent's steps are shown as tool cards (name, arguments, status, expandable result) between its replies. Tool failures are given back to the model as results.
- **Steering:** send a message while an agent works. It waits for the next safe step; a reply that is streaming is interrupted and kept (marked Interrupted) and the same run continues. Stop ends the run. A step budget (default 20, 1 to 100) ends a run that keeps calling tools.
- The agent's folder, step budget and tools can be changed while it is idle (Tools menu; the folder button). Incognito agents are kept in memory only.
- The workspace jail: tool paths cannot leave the folder through absolute paths, `..`, NUL, or symlinks, and `.env`, `.env.*`, `*.pem`, `*.key`, `.ssh`, `.aws`, `.config/gcloud` and files named `credentials*` or `secrets*` are never readable.
- Menu: View ▸ Work (⌘2; Browse and Installed move to ⌘3 and ⌘4) and File ▸ New Agent….

### Menu bar

- A real menu bar: native on macOS (app menu with About, Settings…, Quit; File, Edit, View, Models, Window, Help) and Windows, and an in-window bar on Linux. File: New Chat and New Incognito Chat. View: Chat, Browse, Installed, Settings and a Theme submenu (four colour families and System/Light/Dark). Models: Unload for each loaded model, or Unload All. Edit: Cut, Copy, Paste, Select All. Help: GitHub and issue links. An About window shows the version.
- Standard shortcuts (⌘N, ⇧⌘N, ⌘, , ⌘1–3, ⌘Q on macOS; the same with Ctrl on Windows and Linux). Quit still goes through the close confirmation.
- On Linux the window's minimum height grows by the bar so the content keeps its tested minimum size.

### Added

- A clear (✕) button in the Browse search field.
- Workspace with two crates: `hf2ollama-core` (library, no UI dependencies) and `hf2ollama-gui` (egui binary).
- Core library: shared event and command types, with a channel roundtrip test.
- Core library: Hugging Face search and model metadata (per-file sizes and SHA-256 for LFS files, GGUF metadata), with the access token redacted from `Debug` output.
- Core library: resumable, verified downloads using HTTP Range, with a check that a `206` response matches the resume offset, bounded header waits and cancellation that races network waits.
- Core library: Ollama API client (version, tags, show, blob HEAD and upload, create with streamed progress, delete).
- Core library: repo compatibility classification (GGUF, safetensors with a supported architecture, unsupported with a reason) and a bundled, data-driven list of supported architectures.
- Core library: fit estimation of a model against a machine profile.
- Core library: hardware detection (RAM, NVIDIA VRAM, Apple Silicon unified memory) with a manual override.
- Core library: settings and portable-mode paths (a `portable` file next to the executable keeps settings, history and logs beside it).
- Core library: staging folders with guaranteed cleanup, startup sweep of orphaned and expired partial folders, and listing and clearing of cached files.
- Core library: import history and Hugging Face token storage in the system keychain.
- Core library: Ollama model name validation.
- Core library: import pipeline (plan, download, verify, upload, create, cleanup) with per-job logs, history records, cancel and pause support, and guaranteed job directory cleanup.
- Core library: job manager with a parallel-download limit, queue, and pause, resume and cancel controls. A create that has already completed wins over a late cancel.
- Recorded Hugging Face and Ollama fixtures for offline tests, and notes on the verified API behavior in `docs/superpowers/notes/api-verification.md`.
- Bundled fonts (Hanken Grotesk, JetBrains Mono) with their SIL Open Font License texts.
- CI: format, clippy and test on macOS, Windows and Linux; a `dev-hooks` clippy and test job; a manual release workflow that builds a universal macOS app and DMG, plus Windows and Linux archives, and uploads them as artifacts.
- Project documentation: README, contributing guide, code of conduct, security policy, architecture, FAQ, development and roadmap documents, and MIT OR Apache-2.0 licenses.
- Desktop app: window shell with a top bar, view navigation, Ollama status pill and status bar showing the staging folder; light and dark themes with the bundled fonts; minimum window size 900 by 640.
- Browse view: search with filters, a result list, lazily loaded repository details, GGUF and safetensors option tables with fit pills, a quantization picker (none, int8, int4 with estimated sizes), default model name rules, and a pinned import plan card. Gated, unsupported and unreachable states are explained, and failed loads have a Retry.
- Model card ("About this model"): the repository README is fetched by core (size-capped, front matter stripped) and drawn as a safe subset of Markdown with parse and draw budgets (no raw HTML, no images, only absolute http(s) links).
- Downloads dock: one row per job with progress, a rolling five-second download speed, step chips, pause, resume, cancel, dismiss, per-job log, auto-dismiss from Settings, and a height cap so Browse keeps its minimum room. Clicking Log opens a five-line, scrollable drawer above the dock; one log is open at a time.
- App icon: a bundled teal download mark, used for the window and (on macOS) the app bundle and DMG volume.
- macOS release packaging: a universal `hf2ollama.app` for Apple Silicon and Intel inside a drag-install `hf2ollama-<version>-macos-universal.dmg`, ad-hoc signed and checked by `packaging/macos/verify.sh` in CI.
- Installed view: models from the Ollama API with the Hugging Face source recorded by this app, a two-segment usage bar (imported with this app, other), sorting and filtering, details, and delete with two-step confirmation.
- Settings view: Ollama address with a connection test, staging folder (read-only check while typing, write probe on commit), delete-after-import, keep partials, parallel downloads, auto-dismiss, Hugging Face endpoint and token (system keychain, checked against Hugging Face), hardware override, theme, and cached files with a two-step Clear. Changes apply live and are saved to the TOML file.
- Core: `HfClient::whoami` (token check with a readable invalid-token error and redaction) and `HfClient::model_card`.
- Startup: orphaned and expired job folders in the staging folder are swept once off the UI thread (only marker-guarded job folders; the report goes to `startup.log`).
- Opt-in end-to-end tests against a real Ollama and the public Hugging Face API (`crates/hf2ollama-core/tests/e2e_real_ollama.rs`), and the acceptance record in `docs/superpowers/notes/acceptance.md`.
- Workspace package metadata: `license = "MIT OR Apache-2.0"`, authors, `rust-version = 1.95`, `publish = false`.
- `cargo-deny` now passes (licenses, bans, advisories, sources) with four documented "unmaintained" advisory ignores.
- Screenshot and trial environment variables: `HF2O_THEME`, `HF2O_SIZE`, `HF2O_VIEW`, `HF2O_QUERY`, `HF2O_SELECT`; the in-memory token store (`HF2O_MEMORY_TOKEN_STORE`, `HF2O_MEMORY_TOKEN`) exists only with the `dev-hooks` cargo feature.

- Downloads: the download step retries up to 3 times (1 s, 2 s, 4 s; a retry-after hint is honoured up to 30 s) after a network error, a stall, a 5xx or a 429, continuing from the partial file; never after a checksum mismatch, a gated repo, other 4xx errors or a cancel. The wait honours cancel and pause, and each retry is logged.
- Closing the window with running downloads shows a confirmation ("Keep running" or "Quit and cancel"); quitting cancels every job and waits at most 3 seconds.
- The model name is checked again right before `create`, the Download button is disabled while another job uses the same name (and right after a click, so a double click starts one job), and a name collision fails with a readable message. Overwriting is still not offered.
- A download that sends more bytes than its declared size is stopped immediately.
- A job task that panics or is stopped now writes a log line and still ends with one terminal event.
- An unreadable settings file is renamed to `hf2ollama.toml.bak` before defaults are used (logged in `startup.log` and shown in the status bar).
- The typed Hugging Face token and its undo history are wiped when leaving Settings and after Save or Clear.
- Sweeping and clearing the staging folder continue past a folder that cannot be removed and report it.

### Fixed

- The Files and About tabs were stacked on top of each other and scrolled away with the page. They are now one row, pinned above the scrolling area, and no longer change width when selected.

### Changed

- The macOS release artifact is now one universal `.dmg` with an `.app` inside instead of two bare-binary `.tar.gz` archives.
- Settings enum values are written in kebab-case (`never`, `one-minute`, `five-minutes`, `one-hour`; `system`, `light`, `dark`). The earlier PascalCase spellings still load.
- Clear cached files in Settings now also removes the files of failed downloads, and the button is enabled whenever anything is listed. The Settings text and the docs no longer promise resuming: failed downloads are kept for 7 days and then removed, and resuming a failed job or after a restart is not supported yet.
- File sizes from the remote are summed with saturating arithmetic.
- Spec and plan aligned with the verified Ollama safetensors `quantize` values and Hugging Face hash rules.
- CI runs on pushes to `main` and on pull requests only (no duplicate runs), and no longer installs `libgtk-3-dev`, which nothing uses.
- Delete in Installed needs a short pause (0.1 s) between arming and confirming, and held keys never confirm.
- The file dialog for the staging folder uses `rfd` with the XDG portal backend on Linux.
- A saved staging folder that is not an absolute path (or contains `..` or starts with `~`) is ignored at startup in favor of the default, with a notice in the status bar.
- Removed the unused `quality_bar` and `fit_pill` widgets.

### Security

- The in-memory token store is compiled only with the `dev-hooks` feature, needs the exact value `1`, prints a warning and is flagged in Settings, so a release binary cannot be switched to it.
- The Hugging Face token lives in the system keychain only, is passed through a redacting type and never appears in `Debug` output; Hugging Face cross-host redirects do not carry it (tested).
- Staging folder changes require an absolute path without `..` or `~`, and the write probe runs on commit with a unique file.
- Model card rendering is bounded (input size, nesting, blocks, spans, table cells, list items) so a hostile README cannot freeze the UI.

# shiftWire quick start

This guide runs shiftWire in WSL (Kali by default) and uses it from Windows. AI agents connect over **MCP** (section 4)
and/or route their own tools **through the proxy**; either way every request shows up live with
the agent's name, and agents are held to the **scope** you set in Settings.

## 1. Start, stop, open

From a Windows terminal in this folder (after building with `scripts/wsl.sh ui`):

```bat
shiftwire open      &:: starts shiftWire if needed and opens the UI in your browser
shiftwire status
shiftwire stop
shiftwire logs -f
```

The UI is `http://127.0.0.1:7777` and the proxy is `127.0.0.1:8080`. Both are reachable from
Windows and from Kali. shiftWire keeps running in the background when you close the browser.

## 2. Trust the CA (once per browser or tool) to see HTTPS

Set a browser to proxy `127.0.0.1:8080`, open **http://shiftwire/** and click *Download the shiftWire CA
certificate*. Install steps for each OS are on that page.
- **Windows:** open the file → Install Certificate → Current User → *Trusted Root Certification
  Authorities*. Edge and Chrome then work.
- **Firefox:** Settings → Certificates → Import.
- **Kali tools:** the CA is at `~/.shiftwire/ca/ca.crt`.
- **Windows' built-in curl** also needs `--ssl-no-revoke`.

Tip: start a separate browser profile just for testing, e.g.
`msedge --user-data-dir=%TEMP%\shiftwire-edge --proxy-server=127.0.0.1:8080`

## 3. Using the UI

| What | How |
|---|---|
| Lasting filters | `+ Hide…` / `+ Show only…` above the table: chips like `⊘ *mozilla*` or `⊘ status:3*`; click to pause, right-click to change, × removes it. Right-click a row → Hide/Only host makes a chip too |
| Clear | hides every request up to the newest `#` (a `#1–N` chip; right-click it to change both ends, e.g. `20-50`); new ones keep coming, × brings them back |
| `#` | each request's own number in the project, given when it starts; filter with `id:>120` |
| Export | `Export HAR` (right of the chips) downloads what History shows |
| Filter | focused when History opens (or press `/`), ↓ jumps into the list; e.g. `host:shop.test status:>=400 -method:OPTIONS tag:idor` |
| Select | click, Ctrl-click, Shift-click; arrows, Home/End |
| Color / clear | `1`–`7` / `0`; double-click a row marks it purple |
| Tag / note | `t` / `n` (or right-click) |
| Repeater | `r` (or right-click → Send to Repeater), then Ctrl+Enter to send; "Follow redirects" (on by default) shows the final response, every step is in History |
| Copy as curl | right-click |
| AI mode | top right: AI sends / AI advises / AI off |
| Request / response | drag the line between them |
| Right-click a request | the same menu everywhere (History, AI Timeline, Site Map, request details, Repeater response): color, tags, note, mark, copy URL/curl, only/hide host, Send to Repeater, Show in History |
| In scope | pill above History and in the Site Map (one setting): only hosts in your scope; on by default once a scope is set |
| Site Map | sorted by requests, folders first; only what exists (endpoints that only answered 404 are left out); click an endpoint for its requests and the full request/response |
| Network (Settings) | interface (this machine / all interfaces / a specific address) and port for the UI and the proxy; saving restarts shiftWire there and moves the page; shows the token / proxy password other machines need |
| Match & Replace (Settings) | rewrite (Replace) or flag (Alert) every proxied request, e.g. force your User-Agent or alert when it is not yours; matches are tagged `replaced:…` / `alert:…` and alerts pop up |
| Projects | click the project name under the logo: create (name, scope, folder), switch, open a folder, delete |

### Projects

Each project is a folder, by default under `~/shiftwire-projects/` in Kali (set `projects_dir` in
`~/.shiftwire/config.toml` to change it): `project.json` (name, created) and `flows.db` (requests,
tags, Repeater tabs, Timeline, scope, AI mode). Copy the folder to back it up or hand it over; open
any project folder from the Projects page. shiftWire starts with the project used last. Agents
always work in the current project and cannot switch. Keep projects on the Linux side: SQLite is
slow and unreliable on `/mnt/c`.

## 4. Connecting an AI agent

**First set the scope** (Settings → Scope): agents may only send requests to hosts listed there.
Anything else is refused and appears in red in History. Your own traffic is never restricted.
The switch in the top right sets what agents may do:
**AI sends** (they send requests themselves, within the scope), **AI advises** (they read History,
the Repeater and the Site Map and *prepare* requests as Repeater tabs; you review and press Send),
**AI off** (everything they send is refused).

**MCP.** For Claude Code running in Kali, add this to the project's `.mcp.json`:
```json
{ "mcpServers": { "shiftwire": { "command": "/home/YOU/shiftwire/target/release/shiftwire", "args": ["mcp", "--agent", "claude"] } } }
```
or use the HTTP endpoint `http://127.0.0.1:7777/mcp?agent=claude`. Tools: `proxy_info`, `get_scope`,
`list_flows`, `get_flow`, `send_request`, `repeat_flow`, `tag_flows`, `set_note`, `log_intent`,
`mark_flow`, `read_operator_messages`, `open_in_repeater`, `prepare_request`, `list_repeater_tabs`,
`get_repeater_tab`, `sitemap`. Prefer MCP for single, deliberate requests;
use the proxy for tools like ffuf, sqlmap or a browser. The **AI Timeline** shows each agent's plan
(`log_intent`), the requests that followed and what it marked; type a message there and the agents
receive it with their next action.
Agents can read the scope but not change it.

**The agent's own tools (curl, ffuf, browsers)**

Give the agent a proxy URL **with its name as the username**. Every request it sends then
appears live in History with that name as a blue actor badge:

```bash
# inside Kali (where an agent like Claude Code would run)
export HTTP_PROXY=http://claude:x@127.0.0.1:8080
export HTTPS_PROXY=http://claude:x@127.0.0.1:8080
export SSL_CERT_FILE=~/.shiftwire/ca/ca.crt          # curl, Python, most tools
export REQUESTS_CA_BUNDLE=~/.shiftwire/ca/ca.crt      # Python requests
export NODE_EXTRA_CA_CERTS=~/.shiftwire/ca/ca.crt     # Node
```

Tell the agent something like: *"Route all HTTP(S) through the proxy in HTTP_PROXY/HTTPS_PROXY.
For ffuf use `-x http://claude:x@127.0.0.1:8080`."* Then filter on `actor:claude` (or `is:ai`).

Agent tips: every request an agent sends after `log_intent` carries that intent's id, so
`tag_flows` / `set_note` with `filter="intent:<id>"` cover exactly that run; `list_flows` pages
back with `before=<id>`; a switch of the AI mode is announced in the agent's next tool result.

## 5. Proxying a VM

By default the proxy only listens on this machine. For another VM:
1. In Kali, edit `~/.shiftwire/config.toml` → `proxy_addr = "0.0.0.0:8080"`, then `shiftwire restart`.
2. WSL2 only forwards `localhost` from Windows. To let a VM reach it, enable mirrored networking
   (`networkingMode=mirrored` in `%UserProfile%\.wslconfig`, then `wsl --shutdown`), or run the
   VM's tools inside Kali itself.

Clients from other machines then need a password: `shiftwire status` shows it (`proxy pw`); use
`http://NAME:PASSWORD@<host>:8080`. Clients on this machine need none.

To open the **UI from another machine**, set `api_addr = "0.0.0.0:7777"`; open it once as
`http://<host>:7777/?token=…` with the token from `shiftwire status` (`UI token`). Agents use
`Authorization: Bearer <token>` for `/mcp`.

## 6. Known limits of this build

- AI vendors' API hosts (`*.anthropic.com`, `*.claude.ai`, `*.openai.com`) are passed through untouched,
  so an agent's own model traffic is never decrypted or stored.
- Clients talk HTTP/1.1 to shiftWire (browsers handle this fine); WebSockets over plain HTTP aren't passed through.
- The UI loads the newest 100,000 requests; filters search the whole database.
- For a clean start, create a new project (or delete an old one on the Projects page).

## Rebuilding after code changes

From Git Bash in this folder: `scripts/wsl.sh ui` (UI + binary), then `shiftwire restart`.

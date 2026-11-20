# <img src="ui/public/favicon.svg" alt="shiftWire" height="36" align="texttop"> &nbsp;shiftWIRE

A small, fast HTTP(S) proxy for **AI pentesting with a human watching**.

Your AI agent sends its requests through shiftWire. You see every one of them live, with the
agent's name on it, in a clean web UI. You can stop the agent, let it only suggest requests, or
let it send them itself, and it can only touch the hosts in your scope.

## shiftWire + shiftGRID

shiftWire is the sibling of [shiftGRID](https://github.com/BuFuuu/shiftgrid).

- **shiftGRID** is the plan: scope, workflows, checklists, notes and findings.
  It tells the agent *what* to test.
- **shiftWire** is the wire: every request the agent sends, live, so you can see *how* it tests.

Point your agent at both. It reads its next step from shiftGRID and sends its traffic through
shiftWire. You watch both in your browser and can take over at any time.

## What you get

- Live history with fast filters, tags, colors and notes
- Repeater to change and resend requests
- Site map of every endpoint seen
- MCP for AI agents, with three modes: AI sends, AI advises, AI off
- An AI timeline: what the agent planned, what it sent, what it found
- HAR export, one folder per project

## Start

```bash
shiftwire start    # run in the background
shiftwire open     # open the UI
shiftwire status   # is it running? URLs and counters
shiftwire stop
```

UI on `http://127.0.0.1:7777`, proxy on `127.0.0.1:8080`, MCP on `http://127.0.0.1:7778/mcp`.
For HTTPS, trust the shiftWire CA: open `http://shiftwire/` through the proxy.

Build, setup and agent config: **[QUICKSTART.md](QUICKSTART.md)**.

## License

MIT. See [LICENSE](LICENSE) and [CONTRIBUTING.md](CONTRIBUTING.md).

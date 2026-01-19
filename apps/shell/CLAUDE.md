# AgentOS Shell

React-based web UI for browsing and managing AgentOS worlds.

## Tech Stack

- **React 19** with **React Router 7**
- **TanStack Query** for data fetching and caching
- **Tailwind CSS v4** via `@tailwindcss/vite`
- **shadcn/ui** components (Radix primitives)
- **Vite 7** with SWC for fast builds
- **TypeScript 5.9**

## Development

```bash
# Start dev server (runs on port 7778)
pnpm dev

# Regenerate API types from running backend
pnpm openapi:types

# Build for production
pnpm build
```

**Backend requirement**: The API server must be running on `localhost:7777`. The Vite dev server proxies `/api/*` requests to it.

## Routes

| Path | Page | Description |
|------|------|-------------|
| `/` | Home | Landing with feature cards |
| `/explorer` | Explorer Overview | World stats, def counts, recent defs |
| `/explorer/manifest` | Manifest | Raw manifest viewer with tabs |
| `/explorer/defs` | Definitions | Filterable defs table |
| `/explorer/defs/:kind/:name` | Def Detail | Single definition view |
| `/explorer/plans/:name` | Plan Diagram | Plan DAG visualization |
| `/workspaces` | Workspaces Index | List of workspace trees |
| `/workspaces/:wsId` | Workspace | Tree file browser |
| `/governance` | Governance Index | Proposals list |
| `/governance/draft` | Draft | New proposal editor |

## Debugging with MCP Browser Tools

Claude Code can interact with the running app via Playwright MCP tools. This enables visual debugging and UI verification.

### Starting a Debug Session

1. Ensure the dev server is running: `pnpm dev` (port 7778)
2. Ensure the API backend is running on port 7777
3. Ask Claude to navigate to the app:
   ```
   Navigate to http://localhost:7778
   ```

### Available MCP Browser Commands

**Navigation:**
- `browser_navigate` - Go to a URL
- `browser_navigate_back` - Go back
- `browser_click` - Click an element by ref
- `browser_type` - Type text into an input

**Inspection:**
- `browser_snapshot` - Get accessibility tree (best for understanding page structure)
- `browser_take_screenshot` - Capture visual screenshot
- `browser_console_messages` - View console logs/errors
- `browser_network_requests` - See API calls made

**Interaction:**
- `browser_fill_form` - Fill multiple form fields
- `browser_select_option` - Select dropdown options
- `browser_press_key` - Press keyboard keys
- `browser_hover` - Hover over elements

### Debug Workflow Examples

**Verify a page renders correctly:**
```
Navigate to http://localhost:7778/explorer
Take a snapshot to see the page structure
```

**Debug an API issue:**
```
Navigate to http://localhost:7778/explorer
Check network requests to see API calls
Check console messages for errors
```

**Test user interactions:**
```
Navigate to http://localhost:7778
Click on the Explorer card
Verify the URL changed to /explorer
Take a snapshot to confirm the page loaded
```

**Inspect element state:**
```
Take a snapshot
Look for the element ref in the output
Use browser_evaluate to inspect its properties
```

### Tips

- Use `browser_snapshot` over `browser_take_screenshot` when you need to interact with elements (snapshots include refs)
- Element refs (e.g., `ref=e32`) from snapshots can be used in subsequent click/type commands
- Network requests show all API calls - useful for debugging data loading issues
- Console messages capture React errors and warnings

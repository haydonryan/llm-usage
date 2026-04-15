# llm-usage

Fetches usage stats for Kimi and Codex (ChatGPT backend), plus OpenAI API billing.

## Requirements

- Rust 1.78+ (edition 2024)
- For Kimi usage: complete the device login flow once
- For Codex usage: a ChatGPT access token or Codex auth file
- For OpenAI API costs: an OpenAI API key

## Usage

## Install

Build and install the binary into `/usr/local/bin`:

```bash
cargo build --release
sudo install -m 0755 target/release/llm-usage /usr/local/bin/llm-usage
```

### Combined usage (default)

```bash
cargo run --release
```

The default command shows Kimi usage only when a Kimi token is available and shows Codex usage only when a ChatGPT access token is available. If either section fails for reasons other than a missing token, the other section will still print and the command will exit non-zero.

To pass a Kimi token directly for the default command:

```bash
cargo run --release -- --kimi-token "$KIMI_TOKEN"
```

To emit JSON instead of the human-readable summary:

```bash
cargo run --release -- --json
```

To show token/debug diagnostics when a service is unavailable:

```bash
cargo run --release -- --debug
```

### Kimi usage

```bash
cargo run --release -- kimi login
cargo run --release -- kimi usage
```

Store a token directly (skips the device login flow):

```bash
cargo run --release -- kimi set-token "$KIMI_TOKEN"
```

After `kimi login`, the Kimi token is saved to `~/.config/llm-usage/llm-usage.toml`.

Optional flags for Kimi usage:

- `--raw` to print the raw JSON response
- `--token TOKEN` to use an access token directly
- `--json` to emit the summarized usage as JSON

Optional environment variables for Kimi:

- `KIMI_CODE_BASE_URL` to override the Kimi Code API base URL
- `KIMI_CODE_OAUTH_HOST` or `KIMI_OAUTH_HOST` to override the OAuth host
- `--debug` to show missing-token and other diagnostic errors in mixed output

### Codex usage limits

```bash
export CHATGPT_ACCESS_TOKEN="your_chatgpt_access_token"
# Optional: export CHATGPT_ACCOUNT_ID="acct_..."
cargo run --release -- codex
```

You can also use the legacy command name:

```bash
cargo run --release -- chatgpt-limits
```

If you are already signed in with Codex, the app will read tokens from `~/.codex/auth.json` when no `CHATGPT_ACCESS_TOKEN` is provided. You can override the path with `--auth-file`.

Optional CLI flags for Codex limits:

- `--access-token` to pass the ChatGPT access token
- `--account-id` to pass the ChatGPT account id
- `--auth-file` to override the Codex auth.json path
- `--base-url` to override the ChatGPT backend base URL
- `--raw` to print the raw JSON response
- `--json` to emit the summarized usage as JSON

### OpenAI API billing costs

```bash
export OPENAI_API_KEY="your_api_key"
cargo run --release -- api-costs
```

Optional environment variables for API costs:

- `OPENAI_ORG` for organization header
- `OPENAI_PROJECT` for project header

Optional CLI flags for API costs:

- `--start YYYY-MM-DD` or `--start RFC3339` to override the usage window
- `--end YYYY-MM-DD` or `--end RFC3339` to override the usage window
- `--raw` to print the raw JSON response
- `--base-url` to target a different OpenAI API base URL
- `--json` to emit the summarized usage as JSON

## Output

### Kimi usage

The app prints a summary per usage bucket (weekly limits and other limits if available):

```
Kimi usage
Limit #1     : [██░░░░░░░░░░░░░░░░░░]  10% used (resets 05:37 on Sun Mar 8)
Weekly limit : [████░░░░░░░░░░░░░░░░]  20% used (resets 19:37 on Sat Mar 14)
Week progress: [-----               ]  25% elapsed
```

### Codex limits

The app prints:

- Plan type (when available)
- A 5h limit line and a weekly limit line with the reset time
- Credits, when the backend reports them

Example:

```
Codex usage limits
Plan: plus
5h limit     : [██████░░░░░░░░░░░░░░]  30% used (resets 22:26 on Sat Mar 14)
Weekly limit : [████████░░░░░░░░░░░░]  40% used (resets 18:26 on Sat Mar 14)
Week progress: [----------          ]  50% elapsed
```

### OpenAI API billing

The app prints:

- The UTC query window
- Total cost so far (from the OpenAI costs endpoint)
- Optional line items if provided by the API
- The next reset time in UTC and local time

The reset time assumes standard calendar-month billing cycles in UTC.

## Notes

- Codex usage limits are fetched from the ChatGPT backend and may change or stop working without notice.

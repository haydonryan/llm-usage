use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Datelike, Local, NaiveDate, SecondsFormat, TimeZone, Utc};
use clap::{Parser, Subcommand};
use directories::{BaseDirs, ProjectDirs};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const DEFAULT_KIMI_OAUTH_HOST: &str = "https://auth.kimi.com";
const DEFAULT_KIMI_BASE_URL: &str = "https://api.kimi.com/coding/v1";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com";
const DEFAULT_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api";
const TOKEN_FILENAME: &str = "token.json";
const DEVICE_ID_FILENAME: &str = "device_id";
const CONFIG_DIR_NAME: &str = "llm-usage";
const CONFIG_FILENAME: &str = "llm-usage.toml";

#[derive(Parser, Debug)]
#[command(
    name = "llm-usage",
    author,
    version,
    about = "Fetch usage stats for Kimi and Codex"
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Kimi access token (used by the default `all` command)
    #[arg(long, global = true, value_name = "TOKEN")]
    kimi_token: Option<String>,
    /// Show additional diagnostics, including missing-token details
    #[arg(long, global = true)]
    debug: bool,
    /// Output summary as JSON
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show both Kimi and Codex usage (default)
    All,
    /// Kimi usage stats and auth
    Kimi {
        #[command(subcommand)]
        command: Option<KimiCommand>,
    },
    /// Codex usage limits from the ChatGPT backend
    #[command(alias = "chatgpt-limits")]
    Codex(ChatgptLimitsArgs),
    /// OpenAI API billing costs for the current calendar month
    ApiCosts(ApiCostsArgs),
}

#[derive(Subcommand, Debug)]
enum KimiCommand {
    /// Login via device authorization flow and store tokens locally
    Login,
    /// Fetch usage from the Kimi Code usage endpoint
    Usage(KimiUsageArgs),
    /// Store a Kimi access token directly
    SetToken(KimiSetTokenArgs),
    /// Remove stored tokens
    Logout,
}

#[derive(Parser, Debug, Clone, Default)]
struct KimiUsageArgs {
    /// Print raw JSON response instead of a summary
    #[arg(long)]
    raw: bool,
    /// Use a provided access token instead of the stored token
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,
}

#[derive(Parser, Debug, Clone)]
struct KimiSetTokenArgs {
    /// Kimi access token to store
    #[arg(value_name = "TOKEN")]
    token: String,
}

#[derive(Parser, Debug, Clone)]
struct ApiCostsArgs {
    /// OpenAI API key (defaults to OPENAI_API_KEY)
    #[arg(long)]
    api_key: Option<String>,
    /// OpenAI organization ID (defaults to OPENAI_ORG)
    #[arg(long)]
    org: Option<String>,
    /// OpenAI project ID (defaults to OPENAI_PROJECT)
    #[arg(long)]
    project: Option<String>,
    /// Base URL for the OpenAI API
    #[arg(long, default_value = DEFAULT_OPENAI_BASE_URL)]
    base_url: String,
    /// Start time (RFC3339) or date (YYYY-MM-DD) for the usage window
    #[arg(long)]
    start: Option<String>,
    /// End time (RFC3339) or date (YYYY-MM-DD) for the usage window
    #[arg(long)]
    end: Option<String>,
    /// Print raw JSON response
    #[arg(long)]
    raw: bool,
}

#[derive(Parser, Debug, Clone)]
struct ChatgptLimitsArgs {
    /// ChatGPT access token (defaults to CHATGPT_ACCESS_TOKEN)
    #[arg(long)]
    access_token: Option<String>,
    /// ChatGPT account id (defaults to CHATGPT_ACCOUNT_ID)
    #[arg(long)]
    account_id: Option<String>,
    /// Path to Codex auth.json (defaults to ~/.codex/auth.json)
    #[arg(long)]
    auth_file: Option<String>,
    /// Base URL for the ChatGPT backend
    #[arg(long, default_value = DEFAULT_CHATGPT_BASE_URL)]
    base_url: String,
    /// Print raw JSON response
    #[arg(long)]
    raw: bool,
}

impl Default for ChatgptLimitsArgs {
    fn default() -> Self {
        Self {
            access_token: None,
            account_id: None,
            auth_file: None,
            base_url: DEFAULT_CHATGPT_BASE_URL.to_string(),
            raw: false,
        }
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    let command = args.command.unwrap_or(Command::All);

    match command {
        Command::All => run_all(args.kimi_token, args.json, args.debug),
        Command::Kimi { command } => {
            let command = command.unwrap_or(KimiCommand::Usage(KimiUsageArgs::default()));
            run_kimi_command(command, args.kimi_token, args.json)
        }
        Command::Codex(cmd_args) => run_chatgpt_limits(cmd_args, true, args.json),
        Command::ApiCosts(cmd_args) => run_api_costs(cmd_args, args.json),
    }
}

const KIMI_TOKEN_MISSING_MESSAGE: &str = "No token found. Run `llm-usage kimi login` first.";

fn run_all(kimi_token: Option<String>, json: bool, debug: bool) -> Result<()> {
    if json {
        return run_all_json(kimi_token, debug);
    }
    let mut failures = Vec::new();

    if kimi_token.is_some() || has_kimi_token_config() {
        let kimi_args = KimiUsageArgs {
            raw: false,
            token: kimi_token,
        };

        if let Err(err) = run_kimi_usage(kimi_args, true, false) {
            if is_kimi_missing_token_error(&err) {
                if debug {
                    eprintln!("Kimi usage unavailable: {err}");
                }
            } else {
                eprintln!("Kimi usage error: {err}");
                failures.push("kimi".to_string());
            }
        }
    }

    println!();

    let codex_args = ChatgptLimitsArgs::default();
    if codex_token_available(&codex_args)
        && let Err(err) = run_chatgpt_limits(codex_args, true, false)
    {
        eprintln!("Codex usage error: {err}");
        failures.push("codex".to_string());
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "usage retrieval failed for: {}",
            failures.join(", ")
        ))
    }
}

fn run_all_json(kimi_token: Option<String>, debug: bool) -> Result<()> {
    let mut failures = Vec::new();
    let mut errors = Vec::new();
    let mut kimi = None;
    let mut codex = None;

    if kimi_token.is_some() || has_kimi_token_config() {
        let kimi_args = KimiUsageArgs {
            raw: false,
            token: kimi_token,
        };

        match fetch_kimi_usage_payload(&kimi_args) {
            Ok(payload) => {
                let rows = collect_kimi_rows(&payload);
                kimi = Some(KimiUsageJson {
                    rows: kimi_rows_to_json(&rows),
                });
            }
            Err(err) => {
                if is_kimi_missing_token_error(&err) {
                    if debug {
                        errors.push(format!("kimi: {err}"));
                    }
                } else {
                    failures.push("kimi".to_string());
                    errors.push(format!("kimi: {err}"));
                }
            }
        }
    }

    let codex_args = ChatgptLimitsArgs::default();
    if codex_token_available(&codex_args) {
        match fetch_chatgpt_limits_body(&codex_args) {
            Ok(body) => match parse_chatgpt_limits_payload(&body) {
                Ok(payload) => {
                    let captured_at = Local::now();
                    codex = Some(build_codex_usage_json(&payload, captured_at));
                }
                Err(err) => {
                    failures.push("codex".to_string());
                    errors.push(format!("codex: {err}"));
                }
            },
            Err(err) => {
                failures.push("codex".to_string());
                errors.push(format!("codex: {err}"));
            }
        }
    }

    let output = AllUsageJson {
        kimi,
        codex,
        errors,
    };
    print_json(&output)?;

    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "usage retrieval failed for: {}",
            failures.join(", ")
        ))
    }
}

fn is_kimi_missing_token_error(err: &anyhow::Error) -> bool {
    err.to_string().contains(KIMI_TOKEN_MISSING_MESSAGE)
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn run_kimi_command(command: KimiCommand, global_token: Option<String>, json: bool) -> Result<()> {
    if json {
        match command {
            KimiCommand::Login | KimiCommand::Logout | KimiCommand::SetToken(_) => {
                return Err(anyhow!("`--json` is only supported for usage commands."));
            }
            KimiCommand::Usage(_) => {}
        }
    }
    match command {
        KimiCommand::Login => kimi_login(),
        KimiCommand::Usage(mut args) => {
            if args.token.is_none() {
                args.token = global_token;
            }
            run_kimi_usage(args, true, json)
        }
        KimiCommand::SetToken(args) => kimi_set_token(args),
        KimiCommand::Logout => kimi_logout(),
    }
}

fn run_kimi_usage(args: KimiUsageArgs, include_header: bool, json: bool) -> Result<()> {
    if json && args.raw {
        return Err(anyhow!("`--json` cannot be combined with `--raw`."));
    }
    if include_header && !args.raw && !json {
        println!("Kimi usage");
    }
    let payload = fetch_kimi_usage_payload(&args)?;

    if args.raw {
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let rows = collect_kimi_rows(&payload);
    if json {
        let output = KimiUsageJson {
            rows: kimi_rows_to_json(&rows),
        };
        print_json(&output)?;
        return Ok(());
    }
    print_kimi_usage_summary(&rows);
    Ok(())
}

fn fetch_kimi_usage_payload(args: &KimiUsageArgs) -> Result<Value> {
    let client = client()?;
    let access_token = if let Some(token) = args.token.as_ref() {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("`--token` cannot be empty."));
        }
        trimmed.to_string()
    } else {
        let device_id = load_or_create_device_id()?;
        let headers = kimi_common_headers(&device_id)?;
        let mut token =
            load_kimi_token_from_config().ok_or_else(|| anyhow!(KIMI_TOKEN_MISSING_MESSAGE))?;
        if token.needs_refresh() {
            token = refresh_token(&client, &headers, &token)?;
            save_token(&token)?;
        }
        token.access_token
    };

    let base_url =
        env::var("KIMI_CODE_BASE_URL").unwrap_or_else(|_| DEFAULT_KIMI_BASE_URL.to_string());
    let url = format!("{}/usages", base_url.trim_end_matches('/'));
    let resp = client.get(url).bearer_auth(&access_token).send()?;
    if resp.status() == StatusCode::UNAUTHORIZED {
        return Err(anyhow!(
            "Authorization failed. Run `llm-usage kimi login` again."
        ));
    }
    if !resp.status().is_success() {
        return Err(anyhow!(
            "Usage request failed with status {}",
            resp.status()
        ));
    }
    let payload: Value = resp.json()?;
    Ok(payload)
}

fn kimi_login() -> Result<()> {
    let client = client()?;
    let device_id = load_or_create_device_id()?;
    let headers = kimi_common_headers(&device_id)?;

    let mut auth = request_device_authorization(&client, &headers)?;
    let verification_url = build_verification_url(&auth);
    println!("Open this URL in your browser and complete login:");
    println!("{}", verification_url);
    println!("User code: {}", auth.user_code);
    if let Some(uri) = auth.verification_uri.as_ref() {
        println!("Verification URI: {}", uri);
    }
    println!("Waiting for authorization...");

    let mut started = now_unix();
    loop {
        if let Some(expires_in) = auth.expires_in
            && now_unix() - started >= expires_in as i64
        {
            println!("Device code expired, restarting login...");
            auth = request_device_authorization(&client, &headers)?;
            let verification_url = build_verification_url(&auth);
            println!("Open this URL in your browser and complete login:");
            println!("{}", verification_url);
            println!("User code: {}", auth.user_code);
            started = now_unix();
        }

        match poll_device_token(&client, &headers, &auth)? {
            PollResult::Success(token) => {
                save_token(&token)?;
                println!("Login successful. Token stored.");
                return Ok(());
            }
            PollResult::Expired => {
                println!("Authorization expired, restarting login...");
                auth = request_device_authorization(&client, &headers)?;
                println!("Open this URL in your browser and complete login:");
                let verification_url = build_verification_url(&auth);
                println!("{}", verification_url);
                println!("User code: {}", auth.user_code);
                started = now_unix();
            }
            PollResult::Pending => {}
        }

        let wait = auth.interval.unwrap_or(5).max(1);
        sleep(Duration::from_secs(wait));
    }
}

fn kimi_set_token(args: KimiSetTokenArgs) -> Result<()> {
    let trimmed = args.token.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Token cannot be empty."));
    }
    let token = StoredToken {
        access_token: trimmed.to_string(),
        refresh_token: String::new(),
        expires_at: 0,
        scope: String::new(),
        token_type: String::new(),
    };
    save_token(&token)?;
    println!("Token stored.");
    Ok(())
}

fn kimi_logout() -> Result<()> {
    let token_path = token_path()?;
    let mut removed = false;
    if token_path.exists() {
        fs::remove_file(&token_path)?;
        removed = true;
    }
    if clear_kimi_token_config()? {
        removed = true;
    }
    if removed {
        println!("Token removed.");
    } else {
        println!("No token found.");
    }
    Ok(())
}

fn client() -> Result<Client> {
    Ok(Client::builder().timeout(Duration::from_secs(30)).build()?)
}

#[derive(Debug, Deserialize)]
struct DeviceAuthorization {
    #[serde(alias = "userCode")]
    user_code: String,
    #[serde(alias = "deviceCode")]
    device_code: String,
    #[serde(alias = "verificationUri")]
    verification_uri: Option<String>,
    #[serde(alias = "verificationUriComplete", default)]
    verification_uri_complete: Option<String>,
    #[serde(alias = "expiresIn")]
    expires_in: Option<u64>,
    #[serde(alias = "interval")]
    interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    scope: String,
    token_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredToken {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    scope: String,
    token_type: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LlmUsageConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    kimi: Option<KimiTokenConfig>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KimiTokenConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_type: Option<String>,
}

#[derive(Debug)]
struct UsageRow {
    label: String,
    used: i64,
    limit: i64,
    reset_hint: Option<String>,
    reset_at: Option<DateTime<Local>>,
}

#[derive(Debug, Serialize)]
struct UsageRowJson {
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    percent_used: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    week_progress_percent: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Debug, Serialize)]
struct KimiUsageJson {
    rows: Vec<UsageRowJson>,
}

#[derive(Debug, Serialize)]
struct CodexUsageJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    components: BTreeMap<String, BTreeMap<String, CodexUsageComponent>>,
    stale: bool,
}

#[derive(Debug, Serialize)]
struct CodexUsageComponent {
    limit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    percent_used: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    week_progress_percent: Option<i64>,
}

#[derive(Debug, Serialize)]
struct AllUsageJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    kimi: Option<KimiUsageJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    codex: Option<CodexUsageJson>,
    errors: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ApiCostsJson {
    start: String,
    end: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    currency: Option<String>,
    line_items: BTreeMap<String, f64>,
    reset_time_utc: String,
    reset_time_local: String,
}

fn request_device_authorization(
    client: &Client,
    headers: &HeaderMap,
) -> Result<DeviceAuthorization> {
    let host = oauth_host();
    let url = format!(
        "{}/api/oauth/device_authorization",
        host.trim_end_matches('/')
    );
    let resp = client
        .post(url)
        .headers(headers.clone())
        .form(&[("client_id", CLIENT_ID)])
        .send()?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("Device authorization failed: {status} {body}"));
    }
    Ok(resp.json()?)
}

enum PollResult {
    Pending,
    Success(StoredToken),
    Expired,
}

fn poll_device_token(
    client: &Client,
    headers: &HeaderMap,
    auth: &DeviceAuthorization,
) -> Result<PollResult> {
    let host = oauth_host();
    let url = format!("{}/api/oauth/token", host.trim_end_matches('/'));
    let resp = client
        .post(url)
        .headers(headers.clone())
        .form(&[
            ("client_id", CLIENT_ID),
            ("device_code", auth.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()?;

    let status = resp.status();
    let payload: Value = resp.json()?;
    if status == StatusCode::OK {
        let token = parse_token_response(&payload)?;
        return Ok(PollResult::Success(token));
    }

    if let Some(code) = payload.get("error").and_then(|v| v.as_str()) {
        if code == "authorization_pending" || code == "slow_down" {
            return Ok(PollResult::Pending);
        }
        if code == "expired_token" {
            return Ok(PollResult::Expired);
        }
        if let Some(desc) = payload.get("error_description").and_then(|v| v.as_str()) {
            return Err(anyhow!("Token polling failed: {code} ({desc})"));
        }
        return Err(anyhow!("Token polling failed: {code}"));
    }

    Err(anyhow!("Token polling failed with unexpected response."))
}

fn refresh_token(client: &Client, headers: &HeaderMap, token: &StoredToken) -> Result<StoredToken> {
    if token.refresh_token.is_empty() {
        return Err(anyhow!(
            "Missing refresh token. Run `llm-usage kimi login` again."
        ));
    }
    let host = oauth_host();
    let url = format!("{}/api/oauth/token", host.trim_end_matches('/'));
    let resp = client
        .post(url)
        .headers(headers.clone())
        .form(&[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", token.refresh_token.as_str()),
        ])
        .send()?;
    if resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::FORBIDDEN {
        return Err(anyhow!(
            "Token refresh unauthorized. Run `llm-usage kimi login` again."
        ));
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("Token refresh failed: {status} {body}"));
    }
    let payload: Value = resp.json()?;
    parse_token_response(&payload)
}

fn parse_token_response(payload: &Value) -> Result<StoredToken> {
    let token: TokenResponse = serde_json::from_value(payload.clone())
        .map_err(|_| anyhow!("Missing token fields in response"))?;
    let expires_at = now_unix() + token.expires_in as i64;
    Ok(StoredToken {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at,
        scope: token.scope,
        token_type: token.token_type,
    })
}

fn token_path() -> Result<PathBuf> {
    let dir = data_dir()?;
    Ok(dir.join(TOKEN_FILENAME))
}

fn device_id_path() -> Result<PathBuf> {
    let dir = data_dir()?;
    Ok(dir.join(DEVICE_ID_FILENAME))
}

fn data_dir() -> Result<PathBuf> {
    if let Some(proj) = ProjectDirs::from("com", "kimi", "kimi-usage") {
        let dir = proj.data_dir().to_path_buf();
        fs::create_dir_all(&dir)?;
        return Ok(dir);
    }
    let fallback = PathBuf::from(".").join(".kimi-usage");
    fs::create_dir_all(&fallback)?;
    Ok(fallback)
}

fn config_dir(create: bool) -> Result<PathBuf> {
    let base = if let Some(base) = BaseDirs::new() {
        base.config_dir().to_path_buf()
    } else if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".config")
    } else {
        PathBuf::from(".").join(".config")
    };
    let dir = base.join(CONFIG_DIR_NAME);
    if create {
        fs::create_dir_all(&dir)?;
    }
    Ok(dir)
}

fn config_path() -> Result<PathBuf> {
    let dir = config_dir(true)?;
    Ok(dir.join(CONFIG_FILENAME))
}

fn config_path_no_create() -> Result<PathBuf> {
    let dir = config_dir(false)?;
    Ok(dir.join(CONFIG_FILENAME))
}

fn load_or_create_device_id() -> Result<String> {
    let path = device_id_path()?;
    if path.exists() {
        return Ok(fs::read_to_string(path)?.trim().to_string());
    }
    let device_id = Uuid::new_v4().simple().to_string();
    write_private_file(&path, device_id.as_bytes())?;
    Ok(device_id)
}

fn load_config() -> Option<LlmUsageConfig> {
    let path = config_path_no_create().ok()?;
    let data = fs::read_to_string(path).ok()?;
    toml::from_str(&data).ok()
}

fn save_config(config: &LlmUsageConfig) -> Result<()> {
    let path = config_path()?;
    let data = toml::to_string_pretty(config)?;
    write_private_file(&path, data.as_bytes())?;
    Ok(())
}

fn load_kimi_token_from_config() -> Option<StoredToken> {
    let config = load_config()?;
    config.kimi.and_then(|token| token.to_token())
}

fn save_kimi_token_config(token: &StoredToken) -> Result<()> {
    let mut config = load_config().unwrap_or_default();
    config.kimi = Some(KimiTokenConfig::from_token(token));
    save_config(&config)
}

fn clear_kimi_token_config() -> Result<bool> {
    let path = config_path_no_create()?;
    if !path.exists() {
        return Ok(false);
    }
    let data = fs::read_to_string(&path)?;
    match toml::from_str::<LlmUsageConfig>(&data) {
        Ok(mut config) => {
            if config.kimi.is_none() {
                return Ok(false);
            }
            config.kimi = None;
            if config.is_empty() {
                fs::remove_file(&path)?;
            } else {
                save_config(&config)?;
            }
            Ok(true)
        }
        Err(_) => {
            fs::remove_file(&path)?;
            Ok(true)
        }
    }
}

fn has_kimi_token_config() -> bool {
    let Some(path) = config_path_no_create().ok() else {
        return false;
    };
    if !path.exists() {
        return false;
    }
    load_kimi_token_from_config().is_some()
}

fn save_token_json(token: &StoredToken) -> Result<()> {
    let path = token_path()?;
    let data = serde_json::to_vec_pretty(token)?;
    write_private_file(&path, &data)?;
    Ok(())
}

fn save_token(token: &StoredToken) -> Result<()> {
    save_token_json(token)?;
    save_kimi_token_config(token)?;
    Ok(())
}

fn write_private_file(path: &Path, content: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    set_private_permissions(path);
    Ok(())
}

fn set_private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mut perms) = fs::metadata(path).map(|m| m.permissions()) {
            perms.set_mode(0o600);
            let _ = fs::set_permissions(path, perms);
        }
    }
}

fn oauth_host() -> String {
    env::var("KIMI_CODE_OAUTH_HOST")
        .or_else(|_| env::var("KIMI_OAUTH_HOST"))
        .unwrap_or_else(|_| DEFAULT_KIMI_OAUTH_HOST.to_string())
}

fn kimi_common_headers(device_id: &str) -> Result<HeaderMap> {
    let device_name = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let os = os_info::get();
    let device_model = if os.version().to_string().is_empty() {
        format!("{} {}", os.os_type(), env::consts::ARCH)
    } else {
        format!("{} {} {}", os.os_type(), os.version(), env::consts::ARCH)
    };

    let mut headers = HeaderMap::new();
    headers.insert("X-Msh-Platform", header_value("kimi_cli")?);
    headers.insert("X-Msh-Version", header_value(env!("CARGO_PKG_VERSION"))?);
    headers.insert("X-Msh-Device-Name", header_value(&device_name)?);
    headers.insert("X-Msh-Device-Model", header_value(&device_model)?);
    headers.insert("X-Msh-Os-Version", header_value(&os.version().to_string())?);
    headers.insert("X-Msh-Device-Id", header_value(device_id)?);
    Ok(headers)
}

fn header_value(value: &str) -> Result<HeaderValue> {
    let sanitized = ascii_sanitize(value);
    Ok(HeaderValue::from_str(&sanitized)?)
}

fn ascii_sanitize(value: &str) -> String {
    let mut sanitized: String = value.chars().filter(|c| c.is_ascii()).collect();
    sanitized = sanitized.trim().to_string();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs() as i64
}

fn build_verification_url(auth: &DeviceAuthorization) -> String {
    if let Some(complete) = auth.verification_uri_complete.as_ref() {
        let trimmed = complete.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(base) = auth.verification_uri.as_ref() {
        let mut url = base.trim().to_string();
        if !url.contains("user_code=") {
            if url.contains('?') {
                url.push('&');
            } else {
                url.push('?');
            }
            url.push_str("user_code=");
            url.push_str(&auth.user_code);
        }
        return url;
    }
    auth.user_code.clone()
}

impl StoredToken {
    fn needs_refresh(&self) -> bool {
        if self.refresh_token.trim().is_empty() {
            return false;
        }
        if self.expires_at <= 0 {
            return true;
        }
        let now = now_unix();
        self.expires_at <= now + 300
    }
}

impl LlmUsageConfig {
    fn is_empty(&self) -> bool {
        self.kimi.is_none()
    }
}

impl KimiTokenConfig {
    fn from_token(token: &StoredToken) -> Self {
        Self {
            access_token: Some(token.access_token.clone()),
            refresh_token: Some(token.refresh_token.clone()),
            expires_at: Some(token.expires_at),
            scope: Some(token.scope.clone()),
            token_type: Some(token.token_type.clone()),
        }
    }

    fn to_token(&self) -> Option<StoredToken> {
        let access_token = self.access_token.as_ref()?.trim().to_string();
        if access_token.is_empty() {
            return None;
        }
        Some(StoredToken {
            access_token,
            refresh_token: self.refresh_token.clone().unwrap_or_default(),
            expires_at: self.expires_at.unwrap_or(0),
            scope: self.scope.clone().unwrap_or_default(),
            token_type: self.token_type.clone().unwrap_or_default(),
        })
    }
}

fn collect_kimi_rows(payload: &Value) -> Vec<UsageRow> {
    let mut rows = Vec::new();
    if let Some(limits) = payload.get("limits").and_then(|v| v.as_array()) {
        for (idx, item) in limits.iter().enumerate() {
            if let Some(row) = extract_limit_row(item, idx) {
                rows.push(row);
            }
        }
    }
    if let Some(usage) = payload.get("usage").and_then(|v| v.as_object())
        && let Some(row) = to_usage_row(usage, "Weekly limit")
    {
        rows.push(row);
    }
    rows
}

fn print_kimi_usage_summary(rows: &[UsageRow]) {
    if rows.is_empty() {
        println!("No usage data available.");
        return;
    }

    const WEEK_PROGRESS_LABEL: &str = "Week progress";
    let label_width = rows
        .iter()
        .map(|row| row.label.len())
        .chain(std::iter::once(WEEK_PROGRESS_LABEL.len()))
        .max()
        .unwrap_or(0);

    for row in rows {
        let percent = if row.limit > 0 {
            (row.used as f64 / row.limit as f64) * 100.0
        } else {
            0.0
        };
        let percent = percent.clamp(0.0, 100.0);
        let bar = render_status_limit_progress_bar(percent);
        let percent_label = format_used_percent(percent);
        let reset = format_kimi_reset(row.reset_at, row.reset_hint.as_ref());
        let label = format!("{:width$}", row.label, width = label_width);
        println!("{}: {} {}{}", label, bar, percent_label, reset);
        if is_weekly_label(&row.label) {
            let progress = week_progress_percent(Local::now(), row.reset_at);
            let progress_percent = (progress * 100.0).clamp(0.0, 100.0);
            let progress_bar = render_week_progress_bar(progress_percent);
            let label = format!("{:width$}", WEEK_PROGRESS_LABEL, width = label_width);
            let progress_label = format_elapsed_percent(progress_percent);
            println!("{}: {} {}", label, progress_bar, progress_label);
        }
    }
}

fn kimi_rows_to_json(rows: &[UsageRow]) -> Vec<UsageRowJson> {
    rows.iter()
        .map(|row| {
            let percent = if row.limit > 0 {
                (row.used as f64 / row.limit as f64) * 100.0
            } else {
                0.0
            };
            let percent = percent.clamp(0.0, 100.0);
            let reset_at = row.reset_at.as_ref().map(|dt| dt.to_rfc3339());
            let reset_display = format_kimi_reset_text(row.reset_at, row.reset_hint.as_ref());
            let week_progress_percent = if is_weekly_label(&row.label) {
                let progress = week_progress_percent(Local::now(), row.reset_at);
                let progress_percent = (progress * 100.0).clamp(0.0, 100.0);
                Some(rounded_percent_value(progress_percent))
            } else {
                None
            };
            UsageRowJson {
                label: row.label.clone(),
                percent_used: Some(rounded_percent_value(percent)),
                reset_at,
                reset_display,
                week_progress_percent,
                text: None,
            }
        })
        .collect()
}

fn extract_limit_row(item: &Value, idx: usize) -> Option<UsageRow> {
    let item_map = item.as_object()?;
    let detail = item_map
        .get("detail")
        .and_then(|v| v.as_object())
        .unwrap_or(item_map);
    let label = item_map
        .get("name")
        .or_else(|| detail.get("name"))
        .or_else(|| item_map.get("title"))
        .or_else(|| detail.get("title"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Limit #{}", idx + 1));
    to_usage_row(detail, &label)
}

fn to_usage_row(data: &Map<String, Value>, default_label: &str) -> Option<UsageRow> {
    let limit = to_i64(data.get("limit"));
    let mut used = to_i64(data.get("used"));
    if used.is_none()
        && let (Some(limit_val), Some(remaining)) = (limit, to_i64(data.get("remaining")))
    {
        used = Some(limit_val - remaining);
    }
    if limit.is_none() && used.is_none() {
        return None;
    }
    Some(UsageRow {
        label: data
            .get("name")
            .or_else(|| data.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or(default_label)
            .to_string(),
        used: used.unwrap_or(0),
        limit: limit.unwrap_or(0),
        reset_hint: reset_hint(data),
        reset_at: reset_at_timestamp(data),
    })
}

fn reset_hint(data: &Map<String, Value>) -> Option<String> {
    for key in ["reset_at", "resetAt", "reset_time", "resetTime"] {
        if let Some(val) = data.get(key).and_then(|v| v.as_str()) {
            return Some(format!("resets at {}", val));
        }
    }
    None
}

fn format_kimi_reset_text(
    reset_at: Option<DateTime<Local>>,
    reset_hint: Option<&String>,
) -> Option<String> {
    if let Some(reset_at) = reset_at {
        let time = reset_at.format("%H:%M").to_string();
        let day = reset_at.format("%a %b %-d").to_string();
        return Some(format!("resets {time} on {day}"));
    }
    reset_hint.map(|hint| hint.to_string())
}

fn format_kimi_reset(reset_at: Option<DateTime<Local>>, reset_hint: Option<&String>) -> String {
    format_kimi_reset_text(reset_at, reset_hint)
        .map(|text| format!(" ({text})"))
        .unwrap_or_default()
}

fn reset_at_timestamp(data: &Map<String, Value>) -> Option<DateTime<Local>> {
    for key in ["reset_at", "resetAt", "reset_time", "resetTime"] {
        if let Some(val) = data.get(key)
            && let Some(dt) = parse_reset_value(val)
        {
            return Some(dt);
        }
    }
    None
}

fn parse_reset_value(value: &Value) -> Option<DateTime<Local>> {
    match value {
        Value::Number(_) => {
            let raw = to_i64(Some(value))?;
            parse_epoch(raw).map(|dt| dt.with_timezone(&Local))
        }
        Value::String(s) => {
            if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
                return Some(dt.with_timezone(&Local));
            }
            if let Ok(raw) = s.trim().parse::<i64>() {
                return parse_epoch(raw).map(|dt| dt.with_timezone(&Local));
            }
            None
        }
        _ => None,
    }
}

fn parse_epoch(value: i64) -> Option<DateTime<Utc>> {
    let seconds = if value.abs() > 1_000_000_000_000 {
        value / 1_000
    } else {
        value
    };
    DateTime::<Utc>::from_timestamp(seconds, 0)
}

fn to_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|v| match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    })
}

fn is_weekly_label(label: &str) -> bool {
    label.to_lowercase().contains("week")
}

fn week_progress_percent(now: DateTime<Local>, reset_at: Option<DateTime<Local>>) -> f64 {
    let week = chrono::Duration::days(7);
    if let Some(reset_at) = reset_at {
        let start = reset_at - week;
        let elapsed = now.signed_duration_since(start);
        return (elapsed.num_seconds() as f64 / week.num_seconds() as f64).clamp(0.0, 1.0);
    }

    let days_from_monday = now.weekday().num_days_from_monday() as i64;
    let start_naive = now.date_naive() - chrono::Duration::days(days_from_monday);
    let start_naive = match start_naive.and_hms_opt(0, 0, 0) {
        Some(value) => value,
        None => return 0.0,
    };
    let start = Local
        .from_local_datetime(&start_naive)
        .single()
        .or_else(|| Local.from_local_datetime(&start_naive).earliest())
        .unwrap_or(now);
    let elapsed = now.signed_duration_since(start);
    (elapsed.num_seconds() as f64 / week.num_seconds() as f64).clamp(0.0, 1.0)
}

fn render_week_progress_bar(percent_elapsed: f64) -> String {
    const SEGMENTS: usize = 20;
    let ratio = (percent_elapsed / 100.0).clamp(0.0, 1.0);
    let mut filled = (ratio * SEGMENTS as f64).ceil() as usize;
    if percent_elapsed <= 0.0 {
        filled = 0;
    } else if filled == 0 {
        filled = 1;
    }
    let filled = filled.min(SEGMENTS);
    let empty = SEGMENTS.saturating_sub(filled);
    format!("[{}{}]", "-".repeat(filled), " ".repeat(empty))
}

fn run_api_costs(args: ApiCostsArgs, json: bool) -> Result<()> {
    if json && args.raw {
        return Err(anyhow!("`--json` cannot be combined with `--raw`."));
    }
    let api_key = args
        .api_key
        .or_else(|| env::var("OPENAI_API_KEY").ok())
        .context("missing API key (set --api-key or OPENAI_API_KEY)")?;
    let org = args.org.or_else(|| env::var("OPENAI_ORG").ok());
    let project = args.project.or_else(|| env::var("OPENAI_PROJECT").ok());

    let now = Utc::now();
    let (month_start, next_month_start) = month_bounds_utc(now);

    let start = match args.start {
        Some(value) => parse_datetime(&value)?,
        None => month_start,
    };
    let end = match args.end {
        Some(value) => parse_datetime(&value)?,
        None => now,
    };

    if end < start {
        return Err(anyhow!(
            "end time {} is before start time {}",
            format_time(end),
            format_time(start)
        ));
    }

    let base_url = args.base_url.trim_end_matches('/');
    let url = format!("{base_url}/v1/organization/costs");

    let client = client()?;
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", api_key))?,
    );
    if let Some(org) = org {
        headers.insert("OpenAI-Organization", HeaderValue::from_str(&org)?);
    }
    if let Some(project) = project {
        headers.insert("OpenAI-Project", HeaderValue::from_str(&project)?);
    }

    let response = client
        .get(url)
        .headers(headers)
        .query(&[
            ("start_time", start.timestamp().to_string()),
            ("end_time", end.timestamp().to_string()),
            ("interval", "1d".to_string()),
        ])
        .send()
        .context("failed to call OpenAI costs endpoint")?;

    let status = response.status();
    let body = response.text().context("failed to read response")?;
    if !status.is_success() {
        return Err(anyhow!("OpenAI API error ({status}): {body}"));
    }

    let value: Value = serde_json::from_str(&body).context("invalid JSON response")?;
    if args.raw {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    let summary = summarize_costs(&value);

    if json {
        let output = ApiCostsJson {
            start: format_time(start),
            end: format_time(end),
            total_cost: summary.total_cost,
            currency: summary.currency.clone(),
            line_items: summary.line_items.clone(),
            reset_time_utc: format_time(next_month_start),
            reset_time_local: format_time(next_month_start.with_timezone(&Local)),
        };
        print_json(&output)?;
        return Ok(());
    }

    println!("OpenAI plan usage (billing)");
    println!(
        "Query window (UTC): {} to {}",
        format_time(start),
        format_time(end)
    );
    match summary.total_cost {
        Some(total) => {
            let currency = summary.currency.as_deref().unwrap_or("usd").to_lowercase();
            println!(
                "Total cost so far: {}",
                format_money(total, currency.as_str())
            );
        }
        None => println!("Total cost so far: unavailable (use --raw to inspect)."),
    }

    if !summary.line_items.is_empty() {
        println!("Line items:");
        let line_currency = summary.currency.as_deref().unwrap_or("usd");
        for (name, amount) in &summary.line_items {
            println!("- {}: {}", name, format_money(*amount, line_currency));
        }
    }

    println!("Reset time (UTC): {}", format_time(next_month_start));
    println!(
        "Reset time (local): {}",
        format_time(next_month_start.with_timezone(&Local))
    );
    println!("Note: reset time assumes calendar-month billing in UTC.");

    Ok(())
}

fn run_chatgpt_limits(args: ChatgptLimitsArgs, include_header: bool, json: bool) -> Result<()> {
    if json && args.raw {
        return Err(anyhow!("`--json` cannot be combined with `--raw`."));
    }
    if include_header && !args.raw && !json {
        println!("Codex usage limits");
    }
    let body = fetch_chatgpt_limits_body(&args)?;

    if args.raw {
        println!("{body}");
        return Ok(());
    }
    let captured_at = Local::now();
    let payload = parse_chatgpt_limits_payload(&body)?;
    let snapshots = snapshots_from_payload(&payload, captured_at);
    if json {
        let output = build_codex_usage_json(&payload, captured_at);
        print_json(&output)?;
        return Ok(());
    }
    if snapshots.is_empty() {
        println!("No rate limit data available.");
        return Ok(());
    }

    if let Some(plan) = payload.plan_type.as_deref() {
        println!("Plan: {}", plan);
    }
    for line in render_rate_limit_lines(&snapshots, captured_at) {
        println!("{line}");
    }
    if is_stale(&snapshots, captured_at) {
        println!("Warning: limits may be stale - start a new turn to refresh.");
    }
    Ok(())
}

fn fetch_chatgpt_limits_body(args: &ChatgptLimitsArgs) -> Result<String> {
    let mut access_token = args
        .access_token
        .clone()
        .or_else(|| env::var("CHATGPT_ACCESS_TOKEN").ok());
    let mut account_id = args
        .account_id
        .clone()
        .or_else(|| env::var("CHATGPT_ACCOUNT_ID").ok());

    if (access_token.is_none() || account_id.is_none())
        && let Some(auth) = load_codex_auth(args.auth_file.as_deref())?
    {
        if access_token.is_none() {
            access_token = auth
                .tokens
                .as_ref()
                .and_then(|tokens| tokens.access_token.clone());
        }
        if account_id.is_none() {
            account_id = auth
                .tokens
                .as_ref()
                .and_then(|tokens| tokens.account_id.clone());
        }
    }

    let access_token = access_token.context(
        "missing ChatGPT access token (set --access-token, CHATGPT_ACCESS_TOKEN, or ~/.codex/auth.json)",
    )?;

    let base_url = args.base_url.trim_end_matches('/');
    let url = format!("{base_url}/wham/usage");

    let client = client()?;
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", access_token))?,
    );
    headers.insert(USER_AGENT, HeaderValue::from_static("llm-usage"));
    if let Some(account_id) = account_id {
        headers.insert("ChatGPT-Account-Id", HeaderValue::from_str(&account_id)?);
    }

    let response = client
        .get(url)
        .headers(headers)
        .send()
        .context("failed to call ChatGPT usage endpoint")?;

    let status = response.status();
    let body = response.text().context("failed to read response")?;
    if !status.is_success() {
        return Err(anyhow!("ChatGPT usage error ({status}): {body}"));
    }
    Ok(body)
}

fn codex_token_available(args: &ChatgptLimitsArgs) -> bool {
    if args
        .access_token
        .as_deref()
        .map(str::trim)
        .is_some_and(|token| !token.is_empty())
    {
        return true;
    }

    if env::var("CHATGPT_ACCESS_TOKEN")
        .ok()
        .map(|token| !token.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }

    load_codex_auth(args.auth_file.as_deref())
        .ok()
        .flatten()
        .and_then(|auth| auth.tokens)
        .and_then(|tokens| tokens.access_token)
        .map(|token| !token.trim().is_empty())
        .unwrap_or(false)
}

fn parse_chatgpt_limits_payload(body: &str) -> Result<RateLimitStatusPayload> {
    serde_json::from_str(body).context("invalid ChatGPT usage JSON response")
}

#[derive(Debug)]
struct CostSummary {
    total_cost: Option<f64>,
    currency: Option<String>,
    line_items: BTreeMap<String, f64>,
}

fn summarize_costs(value: &Value) -> CostSummary {
    let mut total_cost = 0.0;
    let mut saw_cost = false;
    let mut currency: Option<String> = None;
    let mut line_items: BTreeMap<String, f64> = BTreeMap::new();

    if let Some(data) = value.get("data").and_then(|v| v.as_array()) {
        for bucket in data {
            let mut bucket_total_used = false;
            if let Some((amount, cur)) = bucket.get("total_cost").and_then(extract_amount) {
                total_cost += amount;
                saw_cost = true;
                bucket_total_used = true;
                if currency.is_none() {
                    currency = cur;
                }
            }

            if !bucket_total_used
                && let Some((amount, cur)) = bucket.get("amount").and_then(extract_amount)
            {
                total_cost += amount;
                saw_cost = true;
                bucket_total_used = true;
                if currency.is_none() {
                    currency = cur;
                }
            }

            if let Some(items) = bucket.get("line_items").and_then(|v| v.as_array()) {
                let mut bucket_line_total = 0.0;
                let mut saw_line_items = false;
                for item in items {
                    let amount = item
                        .get("cost")
                        .and_then(extract_amount)
                        .or_else(|| item.get("amount").and_then(extract_amount));
                    if let Some((amount, cur)) = amount {
                        let name = item
                            .get("name")
                            .and_then(|v| v.as_str())
                            .or_else(|| item.get("type").and_then(|v| v.as_str()))
                            .unwrap_or("unknown");
                        *line_items.entry(name.to_string()).or_insert(0.0) += amount;
                        bucket_line_total += amount;
                        saw_line_items = true;
                        if currency.is_none() {
                            currency = cur;
                        }
                    }
                }
                if saw_line_items {
                    saw_cost = true;
                    if !bucket_total_used {
                        total_cost += bucket_line_total;
                    }
                }
            }
        }
    }

    if !saw_cost {
        total_cost = 0.0;
    }

    CostSummary {
        total_cost: if saw_cost { Some(total_cost) } else { None },
        currency,
        line_items,
    }
}

fn extract_amount(value: &Value) -> Option<(f64, Option<String>)> {
    match value {
        Value::Number(number) => number.as_f64().map(|v| (v, None)),
        Value::Object(map) => {
            let mut currency = map
                .get("currency")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string());

            if let Some(val) = map.get("value").and_then(|v| v.as_f64()) {
                return Some((val, currency));
            }

            if let Some(val) = map.get("amount").and_then(|v| v.as_f64()) {
                return Some((val, currency));
            }

            if let Some((val, cur)) = map.get("amount").and_then(extract_amount) {
                if currency.is_none() {
                    currency = cur;
                }
                return Some((val, currency));
            }

            if let Some((val, cur)) = map.get("total_cost").and_then(extract_amount) {
                if currency.is_none() {
                    currency = cur;
                }
                return Some((val, currency));
            }

            None
        }
        _ => None,
    }
}

fn parse_datetime(input: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Ok(dt.with_timezone(&Utc));
    }

    let date = NaiveDate::parse_from_str(input, "%Y-%m-%d")
        .with_context(|| format!("invalid date or time: {input}"))?;
    Ok(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap()))
}

fn month_bounds_utc(now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let year = now.year();
    let month = now.month();
    let start = Utc
        .with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .expect("valid month start");
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next_start = Utc
        .with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
        .single()
        .expect("valid month start");
    (start, next_start)
}

fn format_time<Tz: TimeZone>(dt: DateTime<Tz>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn format_money(amount: f64, currency: &str) -> String {
    let currency = currency.to_lowercase();
    match currency.as_str() {
        "usd" => format!("${amount:.4} USD"),
        "eur" => format!("€{amount:.4} EUR"),
        "gbp" => format!("£{amount:.4} GBP"),
        other => format!("{amount:.4} {other}"),
    }
}

#[derive(Debug, Deserialize)]
struct RateLimitStatusPayload {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<RateLimitStatusDetails>,
    #[serde(default)]
    credits: Option<CreditStatusDetails>,
    #[serde(default)]
    additional_rate_limits: Option<Vec<AdditionalRateLimitDetails>>,
}

#[derive(Debug, Deserialize)]
struct CodexAuthFile {
    #[serde(default)]
    tokens: Option<CodexAuthTokens>,
}

#[derive(Debug, Deserialize)]
struct CodexAuthTokens {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RateLimitStatusDetails {
    #[serde(default)]
    primary_window: Option<RateLimitWindowSnapshot>,
    #[serde(default)]
    secondary_window: Option<RateLimitWindowSnapshot>,
}

#[derive(Debug, Deserialize)]
struct RateLimitWindowSnapshot {
    used_percent: f64,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AdditionalRateLimitDetails {
    limit_name: String,
    #[serde(default)]
    rate_limit: Option<RateLimitStatusDetails>,
}

#[derive(Debug, Deserialize)]
struct CreditStatusDetails {
    has_credits: bool,
    unlimited: bool,
    #[serde(default)]
    balance: Option<String>,
}

#[derive(Debug, Clone)]
struct RateLimitWindowDisplay {
    used_percent: f64,
    resets_at: Option<String>,
    reset_at: Option<DateTime<Local>>,
    window_minutes: Option<i64>,
}

#[derive(Debug, Clone)]
struct CreditsSnapshotDisplay {
    has_credits: bool,
    unlimited: bool,
    balance: Option<String>,
}

#[derive(Debug, Clone)]
struct RateLimitSnapshotDisplay {
    limit_name: String,
    captured_at: DateTime<Local>,
    primary: Option<RateLimitWindowDisplay>,
    secondary: Option<RateLimitWindowDisplay>,
    credits: Option<CreditsSnapshotDisplay>,
}

fn snapshots_from_payload(
    payload: &RateLimitStatusPayload,
    captured_at: DateTime<Local>,
) -> Vec<RateLimitSnapshotDisplay> {
    let mut snapshots = Vec::new();
    if let Some(details) = payload.rate_limit.as_ref() {
        let credits = payload
            .credits
            .as_ref()
            .map(|credit| CreditsSnapshotDisplay {
                has_credits: credit.has_credits,
                unlimited: credit.unlimited,
                balance: credit.balance.clone(),
            });
        snapshots.push(snapshot_from_details(
            "codex".to_string(),
            details,
            credits,
            captured_at,
        ));
    }

    if let Some(additional) = payload.additional_rate_limits.as_ref() {
        for entry in additional {
            if let Some(details) = entry.rate_limit.as_ref() {
                snapshots.push(snapshot_from_details(
                    entry.limit_name.clone(),
                    details,
                    None,
                    captured_at,
                ));
            }
        }
    }

    snapshots
}

fn snapshot_from_details(
    limit_name: String,
    details: &RateLimitStatusDetails,
    credits: Option<CreditsSnapshotDisplay>,
    captured_at: DateTime<Local>,
) -> RateLimitSnapshotDisplay {
    RateLimitSnapshotDisplay {
        limit_name,
        captured_at,
        primary: details
            .primary_window
            .as_ref()
            .map(|window| window_display(window, captured_at)),
        secondary: details
            .secondary_window
            .as_ref()
            .map(|window| window_display(window, captured_at)),
        credits,
    }
}

fn window_display(
    window: &RateLimitWindowSnapshot,
    captured_at: DateTime<Local>,
) -> RateLimitWindowDisplay {
    let resets_at = window
        .reset_at
        .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
        .map(|dt| dt.with_timezone(&Local))
        .map(|dt| format_reset_timestamp(dt, captured_at));
    let reset_at = window
        .reset_at
        .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
        .map(|dt| dt.with_timezone(&Local));
    let window_minutes = window.limit_window_seconds.and_then(|seconds| {
        if seconds > 0 {
            Some(seconds / 60)
        } else {
            None
        }
    });

    RateLimitWindowDisplay {
        used_percent: window.used_percent,
        resets_at,
        reset_at,
        window_minutes,
    }
}

fn render_rate_limit_lines(
    snapshots: &[RateLimitSnapshotDisplay],
    captured_at: DateTime<Local>,
) -> Vec<String> {
    let mut lines = Vec::new();
    let rows = compose_rate_limit_rows(snapshots, captured_at);
    const WEEK_PROGRESS_LABEL: &str = "Week progress";
    let label_width = rows
        .iter()
        .map(|row| row.label.len())
        .chain(std::iter::once(WEEK_PROGRESS_LABEL.len()))
        .max()
        .unwrap_or(0);
    if rows.is_empty() {
        return vec!["Limits: data not available yet".to_string()];
    }
    for row in rows {
        match row.value {
            StatusRateLimitValue::Window {
                percent_used,
                resets_at,
                reset_at,
            } => {
                let percent_used = percent_used.clamp(0.0, 100.0);
                let bar = render_status_limit_progress_bar(percent_used);
                let summary = format_status_limit_summary(percent_used);
                let label = format!("{:width$}", row.label, width = label_width);
                let mut line = format!("{}: {} {}", label, bar, summary);
                if let Some(resets_at) = resets_at {
                    line.push_str(&format!(" (resets {resets_at})"));
                }
                lines.push(line);
                if is_weekly_label(&row.label) {
                    let progress = week_progress_percent(captured_at, reset_at);
                    let progress_percent = (progress * 100.0).clamp(0.0, 100.0);
                    let progress_bar = render_week_progress_bar(progress_percent);
                    let label = format!("{:width$}", WEEK_PROGRESS_LABEL, width = label_width);
                    let progress_label = format_elapsed_percent(progress_percent);
                    lines.push(format!("{}: {} {}", label, progress_bar, progress_label));
                }
            }
            StatusRateLimitValue::Text(text) => {
                let label = format!("{:width$}", row.label, width = label_width);
                lines.push(format!("{label}: {text}"));
            }
        }
    }
    lines
}

fn build_codex_usage_json(
    payload: &RateLimitStatusPayload,
    captured_at: DateTime<Local>,
) -> CodexUsageJson {
    let snapshots = snapshots_from_payload(payload, captured_at);
    let mut components = BTreeMap::new();
    for snapshot in &snapshots {
        let component_name = if snapshot.limit_name == "codex" {
            payload
                .plan_type
                .as_ref()
                .map(std::string::ToString::to_string)
                .unwrap_or_else(|| snapshot.limit_name.clone())
        } else {
            snapshot.limit_name.clone()
        };
        let mut limits = BTreeMap::new();

        if let Some(primary) = snapshot.primary.as_ref() {
            let key = primary
                .window_minutes
                .map(get_limits_duration)
                .unwrap_or_else(|| "5h".to_string())
                .to_lowercase();
            limits.insert(
                key.clone(),
                codex_window_to_component(&key, primary, captured_at),
            );
        }

        if let Some(secondary) = snapshot.secondary.as_ref() {
            let key = secondary
                .window_minutes
                .map(get_limits_duration)
                .unwrap_or_else(|| "weekly".to_string())
                .to_lowercase();
            let component = codex_window_to_component(&key, secondary, captured_at);
            limits.insert(key.clone(), component);
        }

        if !limits.is_empty() {
            components.insert(component_name, limits);
        }
    }

    CodexUsageJson {
        plan: payload.plan_type.clone(),
        components,
        stale: is_stale(&snapshots, captured_at),
    }
}

fn codex_window_to_component(
    key: &str,
    window: &RateLimitWindowDisplay,
    captured_at: DateTime<Local>,
) -> CodexUsageComponent {
    let percent_used = rounded_percent_value(window.used_percent.clamp(0.0, 100.0));
    let week_progress_percent = if key == "weekly" {
        let progress = week_progress_percent(captured_at, window.reset_at);
        Some(rounded_percent_value((progress * 100.0).clamp(0.0, 100.0)))
    } else {
        None
    };
    CodexUsageComponent {
        limit: key.to_string(),
        percent_used: Some(percent_used),
        reset_at: window.reset_at.map(|dt| dt.to_rfc3339()),
        week_progress_percent,
    }
}

#[derive(Debug, Clone)]
struct StatusRateLimitRow {
    label: String,
    value: StatusRateLimitValue,
}

#[derive(Debug, Clone)]
enum StatusRateLimitValue {
    Window {
        percent_used: f64,
        resets_at: Option<String>,
        reset_at: Option<DateTime<Local>>,
    },
    Text(String),
}

fn compose_rate_limit_rows(
    snapshots: &[RateLimitSnapshotDisplay],
    _now: DateTime<Local>,
) -> Vec<StatusRateLimitRow> {
    let mut rows = Vec::new();
    for snapshot in snapshots {
        let limit_bucket_label = snapshot.limit_name.clone();
        let show_limit_prefix = !limit_bucket_label.eq_ignore_ascii_case("codex");
        let primary_label = snapshot.primary.as_ref().map(|window| {
            window
                .window_minutes
                .map(get_limits_duration)
                .unwrap_or_else(|| "5h".to_string())
        });
        let secondary_label = snapshot.secondary.as_ref().map(|window| {
            window
                .window_minutes
                .map(get_limits_duration)
                .unwrap_or_else(|| "weekly".to_string())
        });
        let window_count =
            usize::from(snapshot.primary.is_some()) + usize::from(snapshot.secondary.is_some());
        let combine_non_codex_single_limit = show_limit_prefix && window_count == 1;

        if show_limit_prefix && !combine_non_codex_single_limit {
            rows.push(StatusRateLimitRow {
                label: format!("{limit_bucket_label} limit"),
                value: StatusRateLimitValue::Text(String::new()),
            });
        }

        if let Some(primary) = snapshot.primary.as_ref() {
            let primary_label =
                format_limit_label(primary_label.clone().unwrap_or_else(|| "5h".to_string()));
            let label = if combine_non_codex_single_limit {
                format!("{} {} limit", limit_bucket_label, primary_label)
            } else {
                format!("{} limit", primary_label)
            };
            rows.push(StatusRateLimitRow {
                label,
                value: StatusRateLimitValue::Window {
                    percent_used: primary.used_percent,
                    resets_at: primary.resets_at.clone(),
                    reset_at: primary.reset_at,
                },
            });
        }

        if let Some(secondary) = snapshot.secondary.as_ref() {
            let secondary_label = format_limit_label(
                secondary_label
                    .clone()
                    .unwrap_or_else(|| "weekly".to_string()),
            );
            let label = if combine_non_codex_single_limit {
                format!("{} {} limit", limit_bucket_label, secondary_label)
            } else {
                format!("{} limit", secondary_label)
            };
            rows.push(StatusRateLimitRow {
                label,
                value: StatusRateLimitValue::Window {
                    percent_used: secondary.used_percent,
                    resets_at: secondary.resets_at.clone(),
                    reset_at: secondary.reset_at,
                },
            });
        }

        if let Some(credits) = snapshot.credits.as_ref()
            && let Some(row) = credit_status_row(credits)
        {
            rows.push(row);
        }
    }

    if rows.is_empty() {
        vec![StatusRateLimitRow {
            label: "Limits".to_string(),
            value: StatusRateLimitValue::Text("data not available yet".to_string()),
        }]
    } else {
        rows
    }
}

fn credit_status_row(credits: &CreditsSnapshotDisplay) -> Option<StatusRateLimitRow> {
    if !credits.has_credits {
        return None;
    }
    if credits.unlimited {
        return Some(StatusRateLimitRow {
            label: "Credits".to_string(),
            value: StatusRateLimitValue::Text("Unlimited".to_string()),
        });
    }
    let balance = credits.balance.as_ref()?;
    let display_balance = format_credit_balance(balance)?;
    Some(StatusRateLimitRow {
        label: "Credits".to_string(),
        value: StatusRateLimitValue::Text(format!("{display_balance} credits")),
    })
}

fn format_credit_balance(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(int_value) = trimmed.parse::<i64>()
        && int_value > 0
    {
        return Some(int_value.to_string());
    }

    if let Ok(value) = trimmed.parse::<f64>()
        && value > 0.0
    {
        let rounded = value.round() as i64;
        return Some(rounded.to_string());
    }

    None
}

fn render_status_limit_progress_bar(percent_used: f64) -> String {
    const SEGMENTS: usize = 20;
    let ratio = (percent_used / 100.0).clamp(0.0, 1.0);
    let filled = (ratio * SEGMENTS as f64).round() as usize;
    let filled = filled.min(SEGMENTS);
    let empty = SEGMENTS.saturating_sub(filled);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

fn format_status_limit_summary(percent_used: f64) -> String {
    format_used_percent(percent_used)
}

fn rounded_percent_value(percent: f64) -> i64 {
    percent.round().clamp(0.0, 100.0) as i64
}

fn format_used_percent(percent_used: f64) -> String {
    let rounded = rounded_percent_value(percent_used);
    format!("{:>3}% used", rounded)
}

fn format_elapsed_percent(percent_elapsed: f64) -> String {
    let rounded = rounded_percent_value(percent_elapsed);
    format!("{:>3}% elapsed", rounded)
}

fn format_reset_timestamp(dt: DateTime<Local>, captured_at: DateTime<Local>) -> String {
    let _ = captured_at;
    dt.format("%H:%M on %a %b %-d").to_string()
}

fn format_limit_label(label: String) -> String {
    match label.as_str() {
        "weekly" => "Weekly".to_string(),
        "monthly" => "Monthly".to_string(),
        "annual" => "Annual".to_string(),
        other => other.to_string(),
    }
}

fn get_limits_duration(window_minutes: i64) -> String {
    const MINUTES_PER_HOUR: i64 = 60;
    const MINUTES_PER_DAY: i64 = 24 * MINUTES_PER_HOUR;
    const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;
    const MINUTES_PER_MONTH: i64 = 30 * MINUTES_PER_DAY;
    const ROUNDING_BIAS_MINUTES: i64 = 3;

    let window_minutes = window_minutes.max(0);

    if window_minutes <= MINUTES_PER_DAY.saturating_add(ROUNDING_BIAS_MINUTES) {
        let adjusted = window_minutes.saturating_add(ROUNDING_BIAS_MINUTES);
        let hours = std::cmp::max(1, adjusted / MINUTES_PER_HOUR);
        format!("{hours}h")
    } else if window_minutes <= MINUTES_PER_WEEK.saturating_add(ROUNDING_BIAS_MINUTES) {
        "weekly".to_string()
    } else if window_minutes <= MINUTES_PER_MONTH.saturating_add(ROUNDING_BIAS_MINUTES) {
        "monthly".to_string()
    } else {
        "annual".to_string()
    }
}

fn is_stale(snapshots: &[RateLimitSnapshotDisplay], now: DateTime<Local>) -> bool {
    const STALE_MINUTES: i64 = 15;
    snapshots.iter().any(|snapshot| {
        now.signed_duration_since(snapshot.captured_at) > chrono::Duration::minutes(STALE_MINUTES)
    })
}

fn load_codex_auth(path: Option<&str>) -> Result<Option<CodexAuthFile>> {
    let path = match path {
        Some(path) => path.to_string(),
        None => {
            let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!("{home}/.codex/auth.json")
        }
    };
    let path = std::path::Path::new(&path);
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let auth: CodexAuthFile =
        serde_json::from_str(&contents).context("invalid JSON in auth file")?;
    Ok(Some(auth))
}

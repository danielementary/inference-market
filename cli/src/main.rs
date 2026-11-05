use std::{
    io::{self, IsTerminal, Write},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use cli::{
    defaults::{self, Prices},
    quote::{self, PriceSource, Quote},
    stream,
};
use common::{
    chain::Chain,
    client::{accounts::Request, instructions::CreateRequestInstructionArgs, types::Model},
    history::{Earnings, RequestRecord},
    ix, keys,
    prompt::{count_tokens, new_salt, prompt_hash},
    protocol::{PromptRequest, StreamEvent},
    Signer,
};

#[derive(Parser)]
#[command(name = "infer", about = "Buy inference from the network, paid in SOL")]
struct Cli {
    /// Keypair name in keys/ used as the client.
    #[arg(long, global = true, default_value = "client-a")]
    key: String,
    #[arg(long, global = true, default_value = "http://127.0.0.1:8080")]
    coordinator: String,
    #[arg(long, global = true, default_value = "http://127.0.0.1:8090")]
    indexer: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the workers' on-chain offers.
    Offers,
    /// Quote, create a request, stream the answer and show the settlement.
    Request(RequestArgs),
    /// Cancel one of your open requests (budget and rent refunded, request fee kept).
    Cancel { nonce: u64 },
    /// List your requests still on-chain (open or assigned).
    Requests,
    /// Your past requests, rebuilt by the indexer (settled and cancelled ones included).
    History,
    /// What a worker earned, from the indexer.
    Earnings {
        /// Keypair name in keys/ of the worker's owner.
        #[arg(long)]
        worker: String,
    },
    /// Show or forget the remembered max prices.
    Prices {
        #[arg(long)]
        reset: bool,
    },
    /// Administration.
    #[command(subcommand)]
    Admin(Admin),
}

#[derive(Args)]
struct RequestArgs {
    #[arg(long, value_enum)]
    model: ModelArg,
    /// Minimum throughput accepted, in tokens per second.
    #[arg(long)]
    min_tps: u32,
    /// Longest answer to pay for, in tokens (sizes the default budget).
    #[arg(long, default_value_t = 200)]
    max_out: u64,
    /// Max lamports per input token (default: last used, else the highest matching offer).
    #[arg(long, requires = "max_price_out")]
    max_price_in: Option<u64>,
    /// Max lamports per output token.
    #[arg(long, requires = "max_price_in")]
    max_price_out: Option<u64>,
    /// Budget in lamports (default: prompt and max_out at the max prices).
    #[arg(long)]
    budget: Option<u64>,
    /// Show the quote and stop.
    #[arg(long)]
    dry_run: bool,
    /// Don't ask for confirmation.
    #[arg(long, short)]
    yes: bool,
    prompt: String,
}

#[derive(Subcommand)]
enum Admin {
    /// Initialize the program config (signed by the upgrade authority).
    Init {
        #[arg(long, default_value_t = 200)]
        fee_bps: u16,
        #[arg(long, default_value_t = 10_000)]
        request_fee: u64,
        /// Keypair name of the upgrade authority.
        #[arg(long, default_value = "admin")]
        admin_key: String,
        /// Keypair name of the coordinator.
        #[arg(long, default_value = "coordinator")]
        coordinator_key: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ModelArg {
    Qwen38,
    Llama4,
}

impl From<ModelArg> for Model {
    fn from(model: ModelArg) -> Self {
        match model {
            ModelArg::Qwen38 => Model::Qwen38,
            ModelArg::Llama4 => Model::Llama4,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let chain = Chain::from_env();
    match cli.command {
        Command::Offers => offers(&chain).await,
        Command::Request(args) => request(&chain, &cli.key, &cli.coordinator, args).await,
        Command::Cancel { nonce } => cancel(&chain, &cli.key, nonce).await,
        Command::Requests => requests(&chain, &cli.key).await,
        Command::History => history(&cli.indexer, &cli.key).await,
        Command::Earnings { worker } => earnings(&cli.indexer, &worker).await,
        Command::Prices { reset } => prices(reset),
        Command::Admin(Admin::Init {
            fee_bps,
            request_fee,
            admin_key,
            coordinator_key,
        }) => init(&chain, &admin_key, &coordinator_key, fee_bps, request_fee).await,
    }
}

async fn offers(chain: &Chain) -> Result<()> {
    let mut workers = chain.workers().await?;
    workers.sort_by_key(|(_, w)| (w.model as u8, w.price_out));
    println!(
        "{:<46} {:<8} {:>6} {:>9} {:>10} ACTIVE",
        "OWNER", "MODEL", "TOK/S", "PRICE IN", "PRICE OUT"
    );
    for (_, w) in workers {
        println!(
            "{:<46} {:<8} {:>6} {:>9} {:>10} {}",
            w.owner.to_string(),
            format!("{:?}", w.model),
            w.tok_per_s,
            w.price_in,
            w.price_out,
            if w.active { "yes" } else { "no" }
        );
    }
    Ok(())
}

async fn request(chain: &Chain, key: &str, coordinator_url: &str, args: RequestArgs) -> Result<()> {
    let client = keys::load(key)?;
    let config = chain
        .config()
        .await?
        .context("Config not initialized (run `infer admin init`)")?;
    let offers: Vec<_> = chain.workers().await?.into_iter().map(|(_, w)| w).collect();
    let model: Model = args.model.into();
    let options = args
        .max_price_in
        .zip(args.max_price_out)
        .map(|(i, o)| Prices {
            max_price_in: i,
            max_price_out: o,
        });
    let q = quote::quote(
        &offers,
        model,
        args.min_tps,
        count_tokens(&args.prompt),
        args.max_out,
        options,
        defaults::load(),
        args.budget,
    )?;
    let rent = chain
        .rpc
        .get_minimum_balance_for_rent_exemption(Request::LEN)
        .await?;
    print_quote(&q, model, config.request_fee, rent);
    if args.dry_run {
        return Ok(());
    }
    if !args.yes && io::stdin().is_terminal() && !confirm("Create the request?")? {
        println!("Cancelled, nothing spent.");
        return Ok(());
    }

    let salt = new_salt();
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let create = ix::create_request(
        &client.pubkey(),
        &config.coordinator,
        CreateRequestInstructionArgs {
            nonce,
            model,
            min_tps: args.min_tps,
            max_price_in: q.prices.max_price_in,
            max_price_out: q.prices.max_price_out,
            budget: q.budget,
            prompt_hash: prompt_hash(&salt, &args.prompt),
        },
    );
    println!(
        "\nrequest {nonce} created ({})",
        chain.send(&[create], &client, &[]).await?
    );

    let body = PromptRequest {
        client: client.pubkey().to_string(),
        nonce,
        prompt: args.prompt,
        salt,
    };
    let http = reqwest::Client::new();
    let mut events = stream::submit(&http, coordinator_url, &body)
        .await
        .with_context(|| {
            format!("the request stays open: `infer cancel {nonce}` refunds its budget")
        })?;
    // Only remember prices a worker can actually accept.
    defaults::save(q.prices)?;
    while let Some(event) = events.next().await {
        print_event(&event.with_context(|| {
            format!(
                "\nlost the connection to the coordinator; request {nonce} is settled when it comes \
                 back (see `infer requests` / `infer history`)"
            )
        })?);
    }
    Ok(())
}

fn print_quote(q: &Quote, model: Model, request_fee: u64, rent: u64) {
    let source = match q.source {
        PriceSource::Options => "from options",
        PriceSource::LastUsed => "last used",
        PriceSource::Offers => "highest matching offer",
    };
    println!("Quote");
    println!(
        "  model         {model:?} ({} eligible offer(s) at these prices)",
        q.eligible
    );
    println!(
        "  max prices    in {} / out {} lamports per token ({source})",
        q.prices.max_price_in, q.prices.max_price_out
    );
    println!(
        "  prompt        {} tokens, answer up to {} tokens",
        q.in_tokens, q.max_out
    );
    println!(
        "  budget        {} lamports (locked, unused part refunded)",
        q.budget
    );
    println!("  request fee   {request_fee} lamports (not refundable)");
    println!("  rent          {rent} lamports (refunded at close)");
    println!("  max cost      {} lamports", q.budget + request_fee);
    if q.eligible == 0 {
        println!(
            "  warning       no offer accepts these prices; the coordinator will refuse the prompt"
        );
    }
}

fn confirm(question: &str) -> Result<bool> {
    print!("{question} [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

fn print_event(event: &StreamEvent) {
    match event {
        StreamEvent::Queued => println!("queued"),
        StreamEvent::Assigned { worker, signature } => {
            println!("assigned to {worker} ({signature})\n")
        }
        StreamEvent::Chunk { token } => {
            print!("{token} ");
            let _ = io::stdout().flush();
        }
        StreamEvent::Settled {
            signature,
            in_tokens,
            out_tokens,
            measured_tps,
            due,
            fee,
            refund,
        } => {
            println!("\n\nsettled ({signature})");
            println!(
                "  tokens        {in_tokens} in / {out_tokens} out, measured {measured_tps} tok/s"
            );
            println!(
                "  paid          {due} lamports (worker {}, coordinator fee {fee})",
                due - fee
            );
            println!("  refunded      {refund} lamports + rent");
        }
        StreamEvent::Error { reason } => println!("\nerror: {reason}"),
    }
}

async fn cancel(chain: &Chain, key: &str, nonce: u64) -> Result<()> {
    let client = keys::load(key)?;
    let request = chain
        .request(&client.pubkey(), nonce)
        .await?
        .with_context(|| format!("no request {nonce} for {}", client.pubkey()))?;
    let signature = chain
        .send(&[ix::cancel_request(&client.pubkey(), nonce)], &client, &[])
        .await?;
    println!(
        "request {nonce} cancelled ({signature}): {} lamports + rent refunded",
        request.budget
    );
    Ok(())
}

async fn requests(chain: &Chain, key: &str) -> Result<()> {
    let client = keys::load(key)?.pubkey();
    let mut mine: Vec<_> = chain
        .requests()
        .await?
        .into_iter()
        .filter(|(_, r)| r.client == client)
        .collect();
    mine.sort_by_key(|(_, r)| r.nonce);
    println!(
        "{:<14} {:<9} {:<8} {:>12} WORKER",
        "NONCE", "STATUS", "MODEL", "BUDGET"
    );
    for (_, r) in mine {
        println!(
            "{:<14} {:<9} {:<8} {:>12} {}",
            r.nonce,
            format!("{:?}", r.status),
            format!("{:?}", r.model),
            r.budget,
            if r.worker == Default::default() {
                "-".into()
            } else {
                r.worker.to_string()
            }
        );
    }
    Ok(())
}

fn prices(reset: bool) -> Result<()> {
    if reset {
        defaults::reset()?;
        println!("remembered prices forgotten");
    } else {
        match defaults::load() {
            Some(p) => println!(
                "max price in {} / out {} lamports per token",
                p.max_price_in, p.max_price_out
            ),
            None => {
                println!("no remembered prices (the next request uses the highest matching offer)")
            }
        }
    }
    Ok(())
}

async fn init(
    chain: &Chain,
    admin_key: &str,
    coordinator_key: &str,
    fee_bps: u16,
    request_fee: u64,
) -> Result<()> {
    let coordinator = keys::load(coordinator_key)?.pubkey();
    if let Some(config) = chain.config().await? {
        // Re-running against an existing config is fine as long as it names this coordinator.
        if config.coordinator != coordinator {
            bail!(
                "already initialized with another coordinator ({})",
                config.coordinator
            );
        }
        println!(
            "already initialized: coordinator {}, fee {} bps, request fee {} lamports",
            config.coordinator, config.fee_bps, config.request_fee
        );
        return Ok(());
    }
    let admin = keys::load(admin_key)?;
    let ix = ix::init_config(&admin.pubkey(), coordinator, fee_bps, request_fee);
    println!(
        "config initialized ({})",
        chain.send(&[ix], &admin, &[]).await?
    );
    println!("  coordinator {coordinator}, fee {fee_bps} bps, request fee {request_fee} lamports");
    Ok(())
}

async fn indexer_get<T: serde::de::DeserializeOwned>(url: String) -> Result<T> {
    let response = reqwest::get(&url)
        .await
        .with_context(|| format!("is the indexer running? ({url})"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("indexer ({status}): {}", response.text().await?);
    }
    Ok(response.json().await?)
}

fn short(address: &str) -> String {
    format!("{}…{}", &address[..6], &address[address.len() - 5..])
}

fn opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or("-".into(), |v| v.to_string())
}

async fn history(indexer: &str, key: &str) -> Result<()> {
    let client = keys::load(key)?.pubkey();
    let records: Vec<RequestRecord> =
        indexer_get(format!("{indexer}/requests?client={client}")).await?;
    println!(
        "{:<20} {:<10} {:<7} {:<13} {:>13} {:>5} {:>8} {:>6} {:>9}",
        "NONCE", "STATUS", "MODEL", "WORKER", "TOKENS IN/OUT", "TOK/S", "PAID", "FEE", "REFUNDED"
    );
    for r in records {
        let tokens = match (r.in_tokens, r.out_tokens) {
            (Some(i), Some(o)) => format!("{i}/{o}"),
            _ => "-".into(),
        };
        println!(
            "{:<20} {:<10} {:<7} {:<13} {:>13} {:>5} {:>8} {:>6} {:>9}",
            r.nonce,
            r.status,
            r.model,
            r.worker.as_deref().map_or("-".into(), short),
            tokens,
            opt(r.measured_tps),
            r.due.unwrap_or(0),
            opt(r.fee),
            opt(r.refund),
        );
    }
    Ok(())
}

async fn earnings(indexer: &str, worker: &str) -> Result<()> {
    let owner = keys::load(worker)?.pubkey();
    let e: Earnings = indexer_get(format!("{indexer}/workers/{owner}/earnings")).await?;
    println!(
        "worker {} (owner {}): {}, {} tok/s, in {} / out {} lamports per token",
        short(&e.worker),
        short(&e.owner),
        e.model,
        e.tok_per_s,
        e.price_in,
        e.price_out
    );
    println!(
        "  jobs          {} ({} paid, {} at zero: SLA missed or no output)",
        e.jobs,
        e.paid_jobs,
        e.jobs - e.paid_jobs
    );
    println!("  tokens        {} in / {} out", e.in_tokens, e.out_tokens);
    println!(
        "  earned        {} lamports (after coordinator fees)",
        e.earned
    );
    if let Some(last) = e.last_job {
        let earned = last.due.unwrap_or(0) - last.fee.unwrap_or(0);
        println!(
            "  last job      request {} for {}: +{earned}",
            last.nonce,
            short(&last.client)
        );
    }
    Ok(())
}

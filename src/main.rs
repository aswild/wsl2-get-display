// literally all of this is because I can't specify fractional timeouts with (OpenBSD) netcat,
// otherwise I'd just use `nc -z -w 0.5 172.23.96.1 6001` in a shell script.
// But since I gotta shell out to another binary anyway, I might as well add the /etc/resolv.conf
// parsing logic here too.

use std::fs;
use std::io::{Cursor, ErrorKind};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::process::{exit, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::Parser;
use serde_json::{self, Value};

/// X11 port number is 6000 plus the display number
const DISPLAY_PORT_OFFSET: u16 = 6000;

/// Lazy global-variable debug logging
static DEBUG: AtomicBool = AtomicBool::new(false);
macro_rules! debug {
    ($($args:tt),+) => {
        if DEBUG.load(Ordering::Relaxed) {
            eprintln!($($args),+);
        }
    };
}

/// Determine the host/hypervisor IP by reading the first nameserver from /etc/resolv.conf
///
/// This is what most basic answers/tutorials online suggest, and it's fine in a default
/// configuration, but won't work in WSL setups that use a custom resolv.conf (e.g. when needing to
/// add search domains or something, or for any other reason don't use the host as WSL's DNS)
fn host_ip_from_resolv_conf() -> Result<IpAddr> {
    String::from_utf8(fs::read("/etc/resolv.conf").context("failed to read /etc/resolv.conf")?)
        .context("/etc/resolv.conf isn't valid utf8")?
        .lines()
        .find_map(|line| {
            let mut words = line.split_ascii_whitespace();
            match (words.next(), words.next()) {
                (Some("nameserver"), Some(addr)) => Some(addr.to_owned()),
                (_, _) => None,
            }
        })
        .ok_or_else(|| anyhow!("unable to find host IP address in /etc/resolv.conf"))?
        .parse::<IpAddr>()
        .context("unable to parse host IP address")
}

/// Determine the host/hypervisor IP by getting the default IPv4 route.
///
/// This parses `ip -4 -json route show default` and extracts the gateway IP address. It should be
/// more reliable than the /etc/resolv.conf method, but could still fail in case a VPN client is
/// running inside the WSL VM or something like that.
///
/// Tested on iproute2 v5.9.0 on ubuntu 21.10. I think the json flag was added in v4.17 which was
/// released in mid-2018, so a somewhat recent distro is needed.
fn host_ip_from_route() -> Result<IpAddr> {
    let mut cmd = Command::new("ip");
    cmd.args(["-4", "-json", "route", "show", "default"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let output = cmd.output().context("failed to execute {cmd:?}")?;
    if !output.status.success() {
        bail!("command {cmd:?} failed");
    }

    // The JSON output should like this. Parse manually rather than pulling in serde derive macros.
    // [
    //   {
    //     "dst": "default",
    //     "gateway": "172.30.192.1",
    //     "dev": "eth0",
    //     "flags": []
    //   }
    // ]
    let js: Value = serde_json::from_reader(Cursor::new(&output.stdout))
        .context("failed to parse output as JSON")?;
    debug!("{cmd:?} returned parsed data:\n{js:#?}");

    // unwrap inner object out of outer array
    let js = match js {
        Value::Array(mut values) => {
            if values.is_empty() {
                bail!("empty json array");
            } else if values.len() > 1 {
                eprintln!("warning: ip route returned multiple defaults routes: {values:?}");
            }
            values.remove(0)
        }
        not_an_array => bail!("expected JSON array, got {not_an_array}"),
    };

    // sanity check, "dst" field should be "default"
    ensure!(
        matches!(js["dst"].as_str(), Some("default")),
        "route destination is not 'default': {js}"
    );

    // extract and parse the gateway field as an IP
    js["gateway"]
        .as_str()
        .ok_or_else(|| anyhow!("default gateway not found (or is not a string)"))?
        .parse::<IpAddr>()
        .context("failed to parse default gateway IP address")
}

/// Find an X server running on the WSL2 host.
///
/// wsl-get-display will infer the WSL2 hypervisor IP by finding the first nameserver in
/// /etc/resolv.conf, then attempts a TCP connection on the appropriate port (6000
/// + display_number)
#[derive(Debug, Parser)]
#[command(version, max_term_width = 80)]
struct Args {
    /// Connection timeout in milliseconds
    #[arg(short, long, default_value = "500")]
    #[arg(value_parser = |s: &str| s.parse().map(Duration::from_millis))]
    timeout: Duration,

    /// Number of retries
    #[arg(short, long, default_value = "1")]
    retries: u16,

    /// Enables verbose debug output on stderr
    #[arg(short, long)]
    verbose: bool,

    /// X display number, e.g. the "1" in "localhost:1"
    #[arg(default_value = "1")]
    display_number: u16,

    /// Use /etc/resolv.conf to determine the host IP address rather than parsing the output of
    /// `ip route`
    #[arg(short = 'R', long)]
    resolv_conf: bool,
}

fn run() -> Result<Option<String>> {
    let args = Args::parse();
    DEBUG.store(args.verbose, Ordering::Relaxed);

    // read /etc/resolv.conf, find the first nameserver, and parse it as an ip address
    let host_ip = if args.resolv_conf { host_ip_from_resolv_conf() } else { host_ip_from_route() }?;

    let port = DISPLAY_PORT_OFFSET
        .checked_add(args.display_number)
        .ok_or_else(|| anyhow!("display offset overflowed max port number"))?;

    let sa = SocketAddr::new(host_ip, port);
    debug!("connecting to {}", sa);

    for retry in 1..=args.retries {
        debug!("connect attempt {}", retry);
        match TcpStream::connect_timeout(&sa, args.timeout) {
            Ok(conn) => {
                debug!("connection succeeded: {:?}", conn);
                return Ok(Some(format!("{}:{}", host_ip, args.display_number)));
                // conn goes out of scope and is dropped, closing the connection
            }

            Err(e) => {
                debug!("connection failed: {}", e);
                match e.kind() {
                    // timeout, retry immediately
                    ErrorKind::TimedOut => (),
                    // connection refused, wait for timeout before retrying
                    ErrorKind::ConnectionRefused => sleep(args.timeout),
                    // bail on any other errors
                    _ => return Err(e.into()),
                }
            }
        }
    }

    debug!("retries exhausted, no server found");
    Ok(None)
}

fn main() {
    match run() {
        Ok(Some(s)) => println!("{}", s),
        Ok(None) => exit(1),
        Err(e) => {
            eprintln!("Error: {:#}", e);
            exit(2);
        }
    };
}

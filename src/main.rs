// literally all of this is because I can't specify fractional timeouts with (OpenBSD) netcat,
// otherwise I'd just use `nc -z -w 0.5 172.23.96.1 6001` in a shell script.
// But since I gotta shell out to another binary anyway, I might as well add the /etc/resolv.conf
// parsing logic here too.

use std::fs;
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::num::ParseIntError;
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use structopt::StructOpt;

/// X11 port number is 6000 plus the display number
const DISPLAY_PORT_OFFSET: u16 = 6000;

/// Lazy global-variable debug logging. Use AtomicBool because accessing static mut variables is
/// unsafe even though we're single-threaded.
static DEBUG: AtomicBool = AtomicBool::new(false);
macro_rules! debug {
    ($($args:tt),+) => {
        if DEBUG.load(Ordering::Relaxed) {
            eprintln!($($args),+);
        }
    };
}

fn parse_duration(s: &str) -> Result<Duration, ParseIntError> {
    Ok(Duration::from_millis(s.parse()?))
}

/// Find an X server running on the WSL2 host.
///
/// wsl-get-display will infer the WSL2 hypervisor IP by finding the first nameserver in
/// /etc/resolv.conf, then attempts a TCP connection on the appropriate port (6000
/// + display_number)
#[derive(Debug, StructOpt)]
#[structopt(max_term_width = 80)]
struct Args {
    /// Connection timeout in milliseconds
    #[structopt(short, long, parse(try_from_str = parse_duration), default_value = "500")]
    timeout: Duration,

    /// Number of retries
    #[structopt(short, long, default_value = "1")]
    retries: u16,

    /// Enables verbose debug output on stderr
    #[structopt(short, long)]
    verbose: bool,

    /// X display number, e.g. the "1" in "localhost:1"
    #[structopt(default_value = "1")]
    display_number: u16,
}

fn run() -> Result<Option<String>> {
    let args = Args::from_args();
    DEBUG.store(args.verbose, Ordering::Relaxed);

    // read /etc/resolv.conf, find the first nameserver, and parse it as an ip address
    let host_ip =
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
            .context("unable to parse host IP address")?;

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

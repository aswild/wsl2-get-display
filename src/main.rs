// literally all of this is because I can't specify fractional timeouts with (OpenBSD) netcat,
// otherwise I'd just use `nc -z -w 0.5 172.23.96.1 6001` in a shell script.
// But since I gotta shell out to another binary anyway, I might as well add the /etc/resolv.conf
// parsing logic here too.

use std::fs;
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::num::ParseIntError;
use std::process::exit;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use structopt::StructOpt;

const DISPLAY_PORT_OFFSET: u16 = 6000;

fn parse_duration(s: &str) -> Result<Duration, ParseIntError> {
    Ok(Duration::from_millis(s.parse()?))
}

/// Find an X server running on the WSL2 host, and return a value for the DISPLAY environment
/// variable.
///
/// wsl-get-display will infer the WSL2 hypervisor IP by finding the first nameserver in
/// /etc/resolv.conf, then attempts a TCP connection on the appropriate port (6000
/// + display_number)
#[derive(Debug, StructOpt)]
#[structopt(max_term_width = 80)]
struct Args {
    /// connection timeout in milliseconds
    #[structopt(short, long, parse(try_from_str = parse_duration), default_value = "500")]
    timeout: Duration,

    /// X display number, e.g. the "1" in "localhost:1"
    #[structopt(default_value = "1")]
    display_number: u16,
}

fn run() -> Result<Option<String>> {
    let args = Args::from_args();

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

    let sa = SocketAddr::new(host_ip, DISPLAY_PORT_OFFSET + args.display_number);

    match TcpStream::connect_timeout(&sa, args.timeout) {
        // yay we connected. the socket will be closed when it goes out of scope and is dropped
        Ok(_) => Ok(Some(format!("{}:{}", host_ip, args.display_number))),
        // hide error messages for timeouts or connection refused or whatever
        Err(e) if matches!(e.kind(), ErrorKind::ConnectionRefused | ErrorKind::TimedOut) => {
            Ok(None)
        }
        // some other error, print that out
        Err(e) => Err(e.into()),
    }
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

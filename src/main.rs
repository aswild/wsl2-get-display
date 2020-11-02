// literally all of this is because I can't specify fractional timeouts with (OpenBSD) netcat,
// otherwise I'd just use `nc -z -w 0.5 172.23.96.1 6001` in a shell script.
// But since I gotta shell out to another binary anyway, I might as well add the /etc/resolv.conf
// parsing logic here too.

use std::env;
use std::io::{self, ErrorKind};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::process::exit;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use getopts::Options;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const DISPLAY_PORT_OFFSET: u16 = 6000;

fn get_host_ip() -> Result<String> {
    String::from_utf8(std::fs::read("/etc/resolv.conf").context("failed to read /etc/resolv.conf")?)
        .context("/etc/resolv.conf isn't valid utf8")?
        .lines()
        .find_map(|line| {
            let mut words = line.split_ascii_whitespace();
            match (words.next(), words.next()) {
                (Some(ns), Some(ip)) if ns == "nameserver" => Some(ip.to_owned()),
                (_, _) => None,
            }
        })
        .ok_or_else(|| anyhow!("unable to get host IP address from /etc/resolv.conf"))
}

fn connect_error_ok(e: &io::Error) -> bool {
    match e.kind() {
        ErrorKind::ConnectionRefused | ErrorKind::TimedOut => true,
        _ => false,
    }
}

fn real_main() -> Result<Option<String>> {
    let mut opts = Options::new();
    opts.optopt("d", "display-num", "X display offset", "OFFSET");
    opts.optflag("h", "help", "print this help text");

    let matches = opts.parse(env::args_os().skip(1))?;
    if matches.opt_present("h") {
        println!("{}", opts.usage("Usage: wsl2-get-display [OPTIONS]"));
        return Ok(None);
    }

    let display_num = match matches.opt_str("d") {
        Some(s) => s.parse::<u16>().context("invalid display number")?,
        None => 1,
    };

    let host_ip = get_host_ip()?;

    let sa = SocketAddr::new(
        host_ip
            .parse::<IpAddr>()
            .context("invalid host IP address")?,
        DISPLAY_PORT_OFFSET + display_num,
    );

    match TcpStream::connect_timeout(&sa, CONNECT_TIMEOUT) {
        // yay we connected. the socket will be closed when it goes out of scope and is dropped
        Ok(_) => Ok(Some(format!("{}:{}", host_ip, display_num))),
        // hide error messages for timeouts or connection refused or whatever
        Err(e) if connect_error_ok(&e) => Ok(None),
        // some other error, print that out
        Err(e) => Err(e.into()),
    }
}

fn main() {
    match real_main() {
        Ok(Some(s)) => {
            println!("{}", s);
        }
        Ok(None) => exit(1),
        Err(e) => {
            eprintln!("Error: {:#}", e);
            exit(1);
        }
    };
}

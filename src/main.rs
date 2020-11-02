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
use clap::{crate_version, App, Arg};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const DISPLAY_PORT_OFFSET: u16 = 6000;

fn get_host_ip() -> Result<String> {
    let resolvconf = std::fs::read("/etc/resolv.conf").context("failed to open /etc/resolv.conf")?;
    let resolvconf = String::from_utf8(resolvconf).context("/etc/resolv.conf isn't valid utf8")?;

    resolvconf
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
    // bleh, I didn't want to have to pull in clap, but at least I turned off all the features
    let args = App::new("wsl2-get-display")
        .version(crate_version!())
        .max_term_width(80)
        .arg(
            Arg::with_name("display_num")
                .short("d")
                .long("display-offset")
                .takes_value(true)
                .default_value("1")
                .help("X display offset"),
        )
        .get_matches();

    // parse and validate display number. safe to unwrap because there's a default value
    let display_num = args
        .value_of("display_num")
        .unwrap()
        .parse::<u16>()
        .context("invalid display number")?;

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

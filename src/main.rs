mod node;

use node::{Node, PeerPids, Register};
use std::path::PathBuf;
use structopt::clap::AppSettings;
use structopt::StructOpt;

#[derive(StructOpt)]
#[structopt(setting = AppSettings::ColorAuto)]
#[structopt(setting = AppSettings::UnifiedHelpMessage)]
#[structopt(setting = AppSettings::DeriveDisplayOrder)]
struct Args {
	/// Source file to execute.
	#[structopt(parse(from_os_str), value_name = "FILE")]
	source: PathBuf,

	/// Process ID of the node left of this one.
	#[structopt(long, value_name = "PID")]
	left: Option<i32>,

	/// Process ID of the node right of this one.
	#[structopt(long, value_name = "PID")]
	right: Option<i32>,

	/// Process ID of the node above this one.
	#[structopt(long, value_name = "PID")]
	up: Option<i32>,

	/// Process ID of the node below this one.
	#[structopt(long, value_name = "PID")]
	down: Option<i32>,
}

fn main() {
	let args = Args::from_args();

	let mut node = Node::new(
		PeerPids {
			left: args.left,
			right: args.right,
			up: args.up,
			down: args.down,
		},
		3, // Next file descriptor after std{in,out,err} is 3.
	);

	eprintln!("PID of this node: {}", std::process::id());

	let x = std::process::id() as i32 % 100;

	// TODO: Execute the program.
	// TODO: Show output/state.

	loop {
		if args.left.is_some() || args.right.is_some() || args.up.is_some() || args.down.is_some() {
			for &i in &[100 + x, 200 + x, 300 + x] {
				eprint!("Sending {}...", i);
				node.write(i, Register::Any);
				eprintln!("done");
			}
		} else {
			dbg!(node.read(Register::Any));
			std::thread::sleep(std::time::Duration::from_secs(1));
		}
	}
}

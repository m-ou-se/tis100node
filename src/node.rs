use arraymap::ArrayMap;
use nix::poll::{poll, PollFd, PollFlags};
use nix::unistd::pipe;
use std::fs::File;
use std::io::{BufRead, BufReader, Lines, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};

const POLLIN: PollFlags = PollFlags::POLLIN;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
	Left,
	Right,
	Up,
	Down,
}

impl Side {
	fn index(self) -> usize {
		self as usize
	}

	fn from_index(i: usize) -> Self {
		use Side::*;
		match i {
			0 => Left,
			1 => Right,
			2 => Up,
			3 => Down,
			_ => unreachable!(),
		}
	}

	fn opposite(self) -> Self {
		use Side::*;
		match self {
			Left => Right,
			Right => Left,
			Up => Down,
			Down => Up,
		}
	}
}

#[derive(Debug)]
struct Pipe {
	read: File,
	write: File,
}

impl Pipe {
	fn new() -> Self {
		let (read, write) = pipe().unwrap();
		unsafe {
			Self {
				read: File::from_raw_fd(read),
				write: File::from_raw_fd(write),
			}
		}
	}
}

#[derive(Debug)]
struct Peer {
	output: File,
	input: Lines<BufReader<File>>,
	input_fd: i32,
	output_read: File, // Just to keep this file descriptor alive.
	input_write: File, // Just to keep this file descriptor alive.
	sent_get: bool,
	got_get: bool,
	cancelled_gets: usize,
}

fn open(path: &str, write: bool) -> File {
	std::fs::OpenOptions::new()
		.read(!write)
		.write(write)
		.open(path)
		.unwrap()
}

impl Peer {
	fn new(side: Side, pid: Option<i32>, fd_offset: i32) -> Self {
		let fd_offset = |side: Side| side.index() as i32 * 4 + fd_offset;
		let (output, input) = if let Some(pid) = pid {
			let offset = fd_offset(side.opposite());
			(
				Pipe {
					read: open(&format!("/proc/{}/fd/{}", pid, offset + 2), false),
					write: open(&format!("/proc/{}/fd/{}", pid, offset + 3), true),
				},
				Pipe {
					read: open(&format!("/proc/{}/fd/{}", pid, offset), false),
					write: open(&format!("/proc/{}/fd/{}", pid, offset + 1), true),
				},
			)
		} else {
			(Pipe::new(), Pipe::new())
		};
		let offset = fd_offset(side);
		assert_eq!(output.read.as_raw_fd(), offset);
		assert_eq!(output.write.as_raw_fd(), offset + 1);
		assert_eq!(input.read.as_raw_fd(), offset + 2);
		assert_eq!(input.write.as_raw_fd(), offset + 3);
		Self {
			output: output.write,
			input_fd: input.read.as_raw_fd(),
			input: BufReader::new(input.read).lines(),
			output_read: output.read,
			input_write: input.write,
			sent_get: false,
			got_get: false,
			cancelled_gets: 0,
		}
	}

	fn send(&mut self, value: i32) {
		while !self.try_send(value) {}
	}

	fn try_send(&mut self, value: i32) -> bool {
		assert!(!self.sent_get);
		if self.got_get {
			self.got_get = false;
		} else {
			match self.input.next().unwrap().unwrap().as_str() {
				"GET" => {}
				x if self.cancelled_gets > 0 && x.parse::<i32>().is_ok() => {
					self.cancelled_gets -= 1;
					return false;
				}
				_ => panic!("unexpected communication"),
			}
		}
		self.output
			.write_all(format!("{}\n", value).as_bytes())
			.unwrap();
		match self.input.next().unwrap().unwrap().as_str() {
			"ACK" => true,
			"NAK" => false,
			x => panic!("unexpected reply {:?}", x),
		}
	}

	fn read(&mut self) -> i32 {
		loop {
			self.request_read();
			if let Some(value) = self.finish_read() {
				return value;
			}
		}
	}

	fn request_read(&mut self) {
		if !self.sent_get {
			self.output.write_all(b"GET\n").unwrap();
			self.sent_get = true;
		}
	}

	fn finish_read(&mut self) -> Option<i32> {
		assert!(self.sent_get);
		match self.input.next().unwrap().unwrap().as_str() {
			"GET" if !self.got_get => {
				self.got_get = true;
				None
			}
			"NAK" if self.got_get => {
				self.got_get = false;
				None
			}
			x => match x.parse::<i32>() {
				Ok(value) => {
					if self.cancelled_gets > 0 {
						self.cancelled_gets -= 1;
						None
					} else {
						self.output.write_all(b"ACK\n").unwrap();
						self.sent_get = false;
						Some(value)
					}
				}
				_ => panic!("unexpected reply"),
			},
		}
	}

	fn cancel_read(&mut self) {
		if self.sent_get {
			self.output.write_all(b"NAK\n").unwrap();
			self.cancelled_gets += 1;
			self.sent_get = false;
		}
	}
}

#[derive(Debug)]
pub struct Node {
	peers: [Peer; 4],
	acc: i32,
	bak: i32,
	last: Option<Side>,
}

#[derive(Debug)]
pub struct PeerPids {
	pub left: Option<i32>,
	pub right: Option<i32>,
	pub up: Option<i32>,
	pub down: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Register {
	Acc,
	Bak,
	Nil,
	Side(Side),
	Any,
	Last,
}

impl Node {
	pub fn new(peers: PeerPids, fd_offset: i32) -> Self {
		Self {
			peers: [
				Peer::new(Side::Left, peers.left, fd_offset),
				Peer::new(Side::Right, peers.right, fd_offset),
				Peer::new(Side::Up, peers.up, fd_offset),
				Peer::new(Side::Down, peers.down, fd_offset),
			],
			acc: 0,
			bak: 0,
			last: None,
		}
	}

	pub fn write(&mut self, value: i32, target: Register) {
		match (target, self.last) {
			(Register::Acc, _) => self.acc = value,
			(Register::Bak, _) => self.bak = value,
			(Register::Nil, _) | (Register::Last, None) => (),
			(Register::Side(s), _) | (Register::Last, Some(s)) => self.peers[s.index()].send(value),
			(Register::Any, _) => self.write_any(value),
		}
	}

	fn write_any(&mut self, value: i32) {
		for i in 0..4 {
			if self.peers[i].got_get {
				if self.peers[i].try_send(value) {
					return;
				}
			}
		}
		let mut fds = self.peers.map(|p| PollFd::new(p.input_fd, POLLIN));
		loop {
			poll(&mut fds, -1).unwrap();
			for i in 0..4 {
				if fds[i].revents().unwrap().contains(POLLIN) {
					if self.peers[i].try_send(value) {
						self.last = Some(Side::from_index(i));
						return;
					}
				}
			}
		}
	}

	pub fn read(&mut self, target: Register) -> i32 {
		match (target, self.last) {
			(Register::Acc, _) => self.acc,
			(Register::Bak, _) => self.bak,
			(Register::Nil, _) | (Register::Last, None) => 0,
			(Register::Side(s), _) | (Register::Last, Some(s)) => self.peers[s.index()].read(),
			(Register::Any, _) => self.read_any(),
		}
	}

	fn read_any(&mut self) -> i32 {
		let mut fds = self.peers.map(|p| PollFd::new(p.input_fd, POLLIN));
		let mut value = None;
		loop {
			for p in &mut self.peers {
				p.request_read();
			}
			poll(&mut fds, -1).unwrap();
			for i in 0..4 {
				if fds[i].revents().unwrap().contains(POLLIN) {
					if let Some(x) = self.peers[i].finish_read() {
						value = Some(x);
						self.last = Some(Side::from_index(i));
						break;
					}
				}
			}
			if let Some(value) = value {
				// Got a value. Cancel all the pending requests.
				for i in 0..4 {
					self.peers[i].cancel_read();
				}
				return value;
			}
		}
	}
}

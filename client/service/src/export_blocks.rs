// Copyright 2017-2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Chain utilities.

#![allow(unused_imports)]

use crate::error;
//use crate::builder::{ServiceBuilderCommand, ServiceBuilder};
use crate::error::Error;
use sc_chain_spec::ChainSpec;
use log::{warn, info};
use futures::{future, prelude::*};
use sp_runtime::traits::{
	Block as BlockT, NumberFor, One, Zero, Header, SaturatedConversion
};
use sp_runtime::generic::{BlockId, SignedBlock};
use codec::{Decode, Encode, IoReader};
use sc_client::{Client, LocalCallExecutor};
use sp_consensus::{
	BlockOrigin,
	import_queue::{IncomingBlock, Link, BlockImportError, BlockImportResult, ImportQueue},
};
use sc_executor::{NativeExecutor, NativeExecutionDispatch};

use std::{io::{Read, Write, Seek}, pin::Pin};
use sc_client_api::BlockBackend;
use std::sync::Arc;

pub fn export_blocks<B, BA, CE, IQ>(
	client: Arc<Client<BA, CE, B, ()>>,
	mut output: impl Write + 'static,
	from: NumberFor<B>,
	to: Option<NumberFor<B>>,
	binary: bool
) -> Pin<Box<dyn Future<Output = Result<(), Error>>>>
where
	B: BlockT,
	BA: sc_client_api::backend::Backend<B> + 'static,
	CE: sc_client_api::call_executor::CallExecutor<B> + Send + Sync + 'static,
	IQ: ImportQueue<B> + Sync + 'static,
{
	let client = client;
	let mut block = from;

	let last = match to {
		Some(v) if v.is_zero() => One::one(),
		Some(v) => v,
		None => client.chain_info().best_number,
	};

	let mut wrote_header = false;

	// Exporting blocks is implemented as a future, because we want the operation to be
	// interruptible.
	//
	// Every time we write a block to the output, the `Future` re-schedules itself and returns
	// `Poll::Pending`.
	// This makes it possible either to interleave other operations in-between the block exports,
	// or to stop the operation completely.
	let export = future::poll_fn(move |cx| {
		if last < block {
			return std::task::Poll::Ready(Err("Invalid block range specified".into()));
		}

		if !wrote_header {
			info!("Exporting blocks from #{} to #{}", block, last);
			if binary {
				let last_: u64 = last.saturated_into::<u64>();
				let block_: u64 = block.saturated_into::<u64>();
				let len: u64 = last_ - block_ + 1;
				output.write_all(&len.encode())?;
			}
			wrote_header = true;
		}

		match client.block(&BlockId::number(block))? {
			Some(block) => {
				if binary {
					output.write_all(&block.encode())?;
				} else {
					serde_json::to_writer(&mut output, &block)
						.map_err(|e| format!("Error writing JSON: {}", e))?;
				}
		},
			// Reached end of the chain.
			None => return std::task::Poll::Ready(Ok(())),
		}
		if (block % 10000.into()).is_zero() {
			info!("#{}", block);
		}
		if block == last {
			return std::task::Poll::Ready(Ok(()));
		}
		block += One::one();

		// Re-schedule the task in order to continue the operation.
		cx.waker().wake_by_ref();
		std::task::Poll::Pending
	});

	Box::pin(export)
}


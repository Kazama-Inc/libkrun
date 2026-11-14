// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//

/// `MuxerRxQ` implements a helper object that `VsockMuxer` can use for queuing RX (host -> guest)
/// packets (or rather instructions on how to build said packets).
///
/// It holds the control packets (connect requests and responses, resets,
/// shutdowns, credit updates) that found no free RX buffer in the guest's
/// queue, in order, until the guest posts more. These packets exist nowhere
/// else, so the queue never drops one: it grows instead (see [`MuxerRxQ::push`]).
use std::collections::VecDeque;

use super::defs;
use super::defs::uapi;
use super::muxer::MuxerRx;
use super::packet::{TsiAcceptRsp, TsiConnectRsp, TsiListenRsp, VsockPacket};

/// The muxer RX queue.
pub struct MuxerRxQ {
    /// The RX queue data.
    q: VecDeque<MuxerRx>,
}

impl MuxerRxQ {
    const SIZE: usize = defs::MUXER_RXQ_SIZE;

    /// Trivial RX queue constructor.
    pub fn new() -> Self {
        Self {
            q: VecDeque::with_capacity(Self::SIZE),
        }
    }

    /// Push a new RX item to the queue. Always succeeds.
    ///
    /// Every item is a control packet (a connect request or response, a
    /// reset, a shutdown, a credit update) that exists nowhere else, so
    /// dropping one leaves its connection waiting forever: a lost response or
    /// credit update hangs whoever is blocked on that connection. The queue
    /// used to refuse items past `SIZE` and its callers ignored the refusal,
    /// so a burst that outran the guest's RX buffers lost packets and,
    /// occasionally, a live connection. It grows instead; `SIZE` is only the
    /// level past which a backlog is worth reporting.
    pub fn push(&mut self, rx: MuxerRx) -> bool {
        self.q.push_back(rx);
        if self.q.len() % Self::SIZE == 0 {
            warn!("vsock RX backlog at {} packets", self.q.len());
        }
        true
    }

    /// Pop an RX item from the front of the queue.
    pub fn pop(&mut self) -> Option<MuxerRx> {
        self.q.pop_front()
    }

    /// Drop all pending RX items. Used on VM restore
    /// to discard packets queued for connections that no longer exist after the
    /// rewind (see [`super::muxer::VsockMuxer::reset_connections`]).
    pub fn clear(&mut self) {
        self.q.clear();
    }

    /// Get the total number of items in the queue.
    pub fn len(&self) -> usize {
        self.q.len()
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub fn rx_to_pkt(cid: u64, rx: MuxerRx, pkt: &mut VsockPacket) {
    match rx {
        MuxerRx::Reset {
            local_port,
            peer_port,
        } => {
            pkt.set_op(uapi::VSOCK_OP_RST)
                .set_src_cid(uapi::VSOCK_HOST_CID)
                .set_dst_cid(cid)
                .set_src_port(local_port)
                .set_dst_port(peer_port)
                .set_len(0)
                .set_type(uapi::VSOCK_TYPE_STREAM)
                .set_flags(0)
                .set_buf_alloc(0)
                .set_fwd_cnt(0);
        }
        MuxerRx::Shutdown {
            local_port,
            peer_port,
            flags,
            fwd_cnt,
        } => {
            // Carry the real buf_alloc/fwd_cnt (not 0): the guest updates its
            // view of our receive window from every packet header, so a zeroed
            // credit here would stall a guest that still wants to send output
            // back after the half-close.
            pkt.set_op(uapi::VSOCK_OP_SHUTDOWN)
                .set_src_cid(uapi::VSOCK_HOST_CID)
                .set_dst_cid(cid)
                .set_src_port(local_port)
                .set_dst_port(peer_port)
                .set_len(0)
                .set_type(uapi::VSOCK_TYPE_STREAM)
                .set_flags(flags)
                .set_buf_alloc(defs::CONN_TX_BUF_SIZE as u32)
                .set_fwd_cnt(fwd_cnt);
        }
        MuxerRx::ConnResponse {
            local_port,
            peer_port,
            result,
        } => {
            pkt.set_op(uapi::VSOCK_OP_RW)
                .set_src_cid(uapi::VSOCK_HOST_CID)
                .set_dst_cid(cid)
                .set_src_port(local_port)
                .set_dst_port(peer_port)
                .set_type(uapi::VSOCK_TYPE_DGRAM);

            pkt.write_connect_rsp(TsiConnectRsp { result });
            pkt.set_len(pkt.buf().unwrap().len() as u32);
        }
        MuxerRx::OpRequest {
            local_port,
            peer_port,
        } => {
            pkt.set_op(uapi::VSOCK_OP_REQUEST)
                .set_src_cid(uapi::VSOCK_HOST_CID)
                .set_dst_cid(cid)
                .set_src_port(local_port)
                .set_dst_port(peer_port)
                .set_type(uapi::VSOCK_TYPE_STREAM)
                .set_buf_alloc(defs::CONN_TX_BUF_SIZE as u32);

            pkt.set_len(0);
        }
        MuxerRx::OpResponse {
            local_port,
            peer_port,
        } => {
            pkt.set_op(uapi::VSOCK_OP_RESPONSE)
                .set_src_cid(uapi::VSOCK_HOST_CID)
                .set_dst_cid(cid)
                .set_src_port(local_port)
                .set_dst_port(peer_port)
                .set_type(uapi::VSOCK_TYPE_STREAM)
                .set_buf_alloc(defs::CONN_TX_BUF_SIZE as u32);

            pkt.set_len(0);
        }
        MuxerRx::GetnameResponse {
            local_port,
            peer_port,
            data,
        } => {
            pkt.set_op(uapi::VSOCK_OP_RW)
                .set_src_cid(uapi::VSOCK_HOST_CID)
                .set_dst_cid(cid)
                .set_src_port(local_port)
                .set_dst_port(peer_port)
                .set_type(uapi::VSOCK_TYPE_DGRAM);

            pkt.write_getname_rsp(data);
            pkt.set_len(pkt.buf().unwrap().len() as u32);
        }
        MuxerRx::CreditRequest {
            local_port,
            peer_port,
            fwd_cnt,
        } => {
            pkt.set_op(uapi::VSOCK_OP_CREDIT_REQUEST)
                .set_src_cid(uapi::VSOCK_HOST_CID)
                .set_dst_cid(cid)
                .set_src_port(local_port)
                .set_dst_port(peer_port)
                .set_type(uapi::VSOCK_TYPE_STREAM)
                .set_buf_alloc(defs::CONN_TX_BUF_SIZE as u32)
                .set_fwd_cnt(fwd_cnt);
        }
        MuxerRx::CreditUpdate {
            local_port,
            peer_port,
            fwd_cnt,
        } => {
            pkt.set_op(uapi::VSOCK_OP_CREDIT_UPDATE)
                .set_src_cid(uapi::VSOCK_HOST_CID)
                .set_dst_cid(cid)
                .set_src_port(local_port)
                .set_dst_port(peer_port)
                .set_type(uapi::VSOCK_TYPE_STREAM)
                .set_buf_alloc(defs::CONN_TX_BUF_SIZE as u32)
                .set_fwd_cnt(fwd_cnt);
        }
        MuxerRx::ListenResponse {
            local_port,
            peer_port,
            result,
        } => {
            pkt.set_op(uapi::VSOCK_OP_RW)
                .set_src_cid(uapi::VSOCK_HOST_CID)
                .set_dst_cid(cid)
                .set_src_port(local_port)
                .set_dst_port(peer_port)
                .set_type(uapi::VSOCK_TYPE_DGRAM);

            pkt.write_listen_rsp(TsiListenRsp { result });
            pkt.set_len(pkt.buf().unwrap().len() as u32);
        }
        MuxerRx::AcceptResponse {
            local_port,
            peer_port,
            result,
        } => {
            pkt.set_op(uapi::VSOCK_OP_RW)
                .set_src_cid(uapi::VSOCK_HOST_CID)
                .set_dst_cid(cid)
                .set_src_port(local_port)
                .set_dst_port(peer_port)
                .set_type(uapi::VSOCK_TYPE_DGRAM);

            pkt.write_accept_rsp(TsiAcceptRsp { result });
            pkt.set_len(pkt.buf().unwrap().len() as u32);
        }
        MuxerRx::DgramDataDnsResponse {
            peer_port,
            data,
            fwd_cnt,
        } => {
            // Deliver as a UDP datagram payload to the guest, mirroring the
            // header that TsiDgramProxy::init_pkt uses for received datagrams.
            pkt.set_op(uapi::VSOCK_OP_RW)
                .set_src_cid(cid)
                .set_dst_cid(uapi::VSOCK_HOST_CID)
                .set_src_port(0)
                .set_dst_port(peer_port)
                .set_type(uapi::VSOCK_TYPE_DGRAM)
                .set_buf_alloc(defs::CONN_TX_BUF_SIZE as u32)
                .set_fwd_cnt(fwd_cnt);

            let len = if let Some(buf) = pkt.buf_mut() {
                let len = std::cmp::min(buf.len(), data.len());
                buf[..len].copy_from_slice(&data[..len]);
                len
            } else {
                0
            };
            pkt.set_len(len as u32);
        }
    }
}

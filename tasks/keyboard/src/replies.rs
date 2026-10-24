//! Kernel slot facts the bridges need before answering a peer.

use hubris_task_slots::SLOTS;

/// Whether `state` is `peer` delivering a message to this task.
///
/// The slot table lists this task among its own entries (`uses-task keyboard`),
/// which is how it can name itself without a syscall.
pub fn is_sending_to_us(state: hubris_abi::TaskState) -> bool {
    state.is_sending_to(hubris_abi::TaskId(u16::from(SLOTS.keyboard)))
}

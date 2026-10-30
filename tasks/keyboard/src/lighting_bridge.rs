use crate::replies;
use hubris_task_slots::SLOTS;
use node75_lighting_api::{GET_STATE, STATE_BYTES, State};
use userlib::{ReplyFaultReason, ResponseCode};

pub fn observer() -> impl FnMut(State) {
    let mut last_update = None;
    move |state: State| {
        // At most one roundtrip per millisecond unless USB or touch state changes.
        let update = (state.now, state.active, state.leds, state.touching);
        if last_update == Some(update) {
            return;
        }
        last_update = Some(update);
        // This highest-priority task cannot be preempted by its client between
        // the status check and receive. Never wait for the renderer.
        let status = kipc::read_task_status(usize::from(SLOTS.lighting.task_index()));
        if !replies::is_sending_to_us(status) {
            return;
        }
        let Ok(message) = userlib::sys_recv_msg(&mut [], Some(SLOTS.lighting)) else {
            return;
        };
        let fault = if message.operation != GET_STATE {
            Some(ReplyFaultReason::UndefinedOperation)
        } else if !matches!(message.data, Ok([])) {
            Some(ReplyFaultReason::BadMessageSize)
        } else if message.lease_count != 0 {
            Some(ReplyFaultReason::BadLeases)
        } else if message.reply_capacity < STATE_BYTES {
            Some(ReplyFaultReason::ReplyBufferTooSmall)
        } else {
            None
        };
        if let Some(fault) = fault {
            userlib::sys_reply_fault(message.sender, fault);
            return;
        }
        userlib::sys_reply(message.sender, ResponseCode::SUCCESS, &state.encode());
    }
}

use crate::replies;
use hubris_task_slots::SLOTS;
use node75_lighting_api::{GET_STATE, STATE_BYTES, State};
use userlib::{ReplyFaultReason, ResponseCode};

pub fn observer() -> impl FnMut([u16; 6], u32, bool, u8) {
    let mut last_update = None;
    move |keys, now, active, leds| {
        // At most one roundtrip per millisecond unless USB state changes.
        if last_update == Some((now, active, leds)) {
            return;
        }
        last_update = Some((now, active, leds));
        // Only this receiver can unblock the sender. Never make USB wait
        // for a faulted or unresponsive renderer.
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
        userlib::sys_reply(
            message.sender,
            ResponseCode::SUCCESS,
            &State {
                keys,
                now,
                active,
                leds,
            }
            .encode(),
        );
    }
}

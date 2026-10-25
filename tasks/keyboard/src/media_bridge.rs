use crate::replies;
use hubris_task_slots::SLOTS;
use node75_touch_api::{MEDIA_KEY, MediaEvent};
use node75_usb::Device;
use userlib::{ReplyFaultReason, ResponseCode};

pub fn receive(device: &mut Device) {
    // Only this receiver can unblock the sender. Never wait for the sensor.
    let status = kipc::read_task_status(usize::from(SLOTS.touch.task_index()));
    if matches!(
        status,
        hubris_abi::TaskState::Faulted { .. }
            | hubris_abi::TaskState::Healthy(hubris_abi::SchedState::Stopped)
    ) {
        device.cancel_consumer();
        return;
    }
    if !replies::is_sending_to_us(status) {
        return;
    }
    let mut bytes = [core::mem::MaybeUninit::uninit(); 2];
    let Ok(message) = userlib::sys_recv_msg(&mut bytes, Some(SLOTS.touch)) else {
        return;
    };
    let fault = if message.operation != MEDIA_KEY {
        Some(ReplyFaultReason::UndefinedOperation)
    } else if message.lease_count != 0 {
        Some(ReplyFaultReason::BadLeases)
    } else if message.reply_capacity < 1 {
        Some(ReplyFaultReason::ReplyBufferTooSmall)
    } else {
        None
    };
    if let Some(fault) = fault {
        userlib::sys_reply_fault(message.sender, fault);
        return;
    }
    let Some(event) = message.data.ok().and_then(MediaEvent::decode) else {
        userlib::sys_reply_fault(message.sender, ReplyFaultReason::BadMessageSize);
        return;
    };
    let usage = if event.pressed { event.key.usage() } else { 0 };
    let accepted = device.enqueue_consumer(usage);
    userlib::sys_reply(message.sender, ResponseCode::SUCCESS, &[u8::from(accepted)]);
}

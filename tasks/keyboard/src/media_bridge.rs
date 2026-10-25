use crate::replies;
use hubris_task_slots::SLOTS;
use node75_touch_api::{CONTACT, MEDIA_KEY, MediaEvent, decode_contact};
use node75_usb::Device;
use userlib::{ReplyFaultReason, ResponseCode};

pub fn receive(device: &mut Device, touching: &mut bool) {
    // Only this receiver can unblock the sender. Never wait for the sensor.
    let status = kipc::read_task_status(usize::from(SLOTS.touch.task_index()));
    if matches!(
        status,
        hubris_abi::TaskState::Faulted { .. }
            | hubris_abi::TaskState::Healthy(hubris_abi::SchedState::Stopped)
    ) {
        device.cancel_consumer();
        *touching = false;
        return;
    }
    if !replies::is_sending_to_us(status) {
        return;
    }
    let mut bytes = [core::mem::MaybeUninit::uninit(); 2];
    let Ok(message) = userlib::sys_recv_msg(&mut bytes, Some(SLOTS.touch)) else {
        return;
    };
    if message.lease_count != 0 {
        userlib::sys_reply_fault(message.sender, ReplyFaultReason::BadLeases);
        return;
    }
    match message.operation {
        MEDIA_KEY => {
            if message.reply_capacity < 1 {
                userlib::sys_reply_fault(message.sender, ReplyFaultReason::ReplyBufferTooSmall);
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
        CONTACT => {
            let Some(contact) = message.data.ok().and_then(decode_contact) else {
                userlib::sys_reply_fault(message.sender, ReplyFaultReason::BadMessageSize);
                return;
            };
            *touching = contact;
            userlib::sys_reply(message.sender, ResponseCode::SUCCESS, &[]);
        }
        _ => userlib::sys_reply_fault(message.sender, ReplyFaultReason::UndefinedOperation),
    }
}

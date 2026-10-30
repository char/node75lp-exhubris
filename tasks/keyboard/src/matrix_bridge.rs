use core::mem::MaybeUninit;

use hubris_task_slots::SLOTS;
use node75_keyboard_core::{MATRIX_SAMPLE, MATRIX_SAMPLE_BYTES, MatrixSample};
use userlib::{Message, MessageOrNotification, ReplyFaultReason, ResponseCode, TaskId};

use crate::replies;

pub enum Event {
    Sample(MatrixSample),
    Lost,
}

pub struct MatrixBridge {
    peer: TaskId,
    received_at: Option<u32>,
}

impl MatrixBridge {
    pub fn new() -> Self {
        Self {
            peer: SLOTS.board,
            received_at: None,
        }
    }

    pub fn available(&self) -> bool {
        self.received_at.is_some()
    }

    pub fn receive(&mut self, now: u32) -> Option<Event> {
        let status = kipc::read_task_status(usize::from(self.peer.task_index()));
        if replies::is_sending_to_us(status) {
            let mut bytes = [MaybeUninit::uninit(); MATRIX_SAMPLE_BYTES];
            match userlib::sys_recv_msg(&mut bytes, Some(self.peer)) {
                Ok(message) => return self.accept(message, now),
                Err(dead) => self.peer = self.peer.with_generation(dead.new_generation()),
            }
        }
        let failed = matches!(
            status,
            hubris_abi::TaskState::Faulted { .. }
                | hubris_abi::TaskState::Healthy(hubris_abi::SchedState::Stopped)
        );
        if self
            .received_at
            .is_some_and(|at| failed || now.wrapping_sub(at) >= 250)
        {
            self.received_at = None;
            return Some(Event::Lost);
        }
        None
    }

    pub fn wait(&mut self, notifications: u32) -> Option<Event> {
        let mut bytes = [MaybeUninit::uninit(); MATRIX_SAMPLE_BYTES];
        match userlib::sys_recv(&mut bytes, notifications, Some(self.peer)) {
            Ok(MessageOrNotification::Message(message)) => {
                let now = userlib::sys_get_timer().now as u32;
                self.accept(message, now)
            }
            Ok(MessageOrNotification::Notification(_)) => None,
            Err(dead) => {
                self.peer = self.peer.with_generation(dead.new_generation());
                None
            }
        }
    }

    fn accept(&mut self, message: Message<'_>, now: u32) -> Option<Event> {
        let fault = if message.operation != MATRIX_SAMPLE {
            Some(ReplyFaultReason::UndefinedOperation)
        } else if message.lease_count != 0 {
            Some(ReplyFaultReason::BadLeases)
        } else {
            None
        };
        if let Some(fault) = fault {
            userlib::sys_reply_fault(message.sender, fault);
        } else if let Some(sample) = message.data.ok().and_then(MatrixSample::decode) {
            userlib::sys_reply(message.sender, ResponseCode::SUCCESS, &[]);
            self.received_at = Some(now);
            return Some(Event::Sample(sample));
        } else {
            userlib::sys_reply_fault(message.sender, ReplyFaultReason::BadMessageSize);
        }
        None
    }
}

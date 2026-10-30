use crate::{GpioWrite, Register};

include!(concat!(env!("OUT_DIR"), "/client.rs"));

mod userlib {
    #[derive(Clone, Copy)]
    pub struct TaskId;
    impl TaskId {
        pub fn gen0(_: u16) -> Self {
            Self
        }
        pub fn with_generation(self, _: ()) -> Self {
            self
        }
    }
    pub struct TaskDeath;
    impl TaskDeath {
        pub fn new_generation(&self) {}
    }
    pub struct ResponseCode;
    impl ResponseCode {
        pub const SUCCESS: Self = Self;
    }

    pub struct Lease {
        attributes: u8,
    }

    impl Lease {
        pub fn read_only(_: &[u8]) -> Self {
            Self { attributes: 1 }
        }

        pub fn write_only(_: &mut [u8]) -> Self {
            Self { attributes: 2 }
        }

        #[allow(dead_code)]
        pub fn read_write(_: &mut [u8]) -> Self {
            Self { attributes: 3 }
        }
    }

    pub fn sys_send(
        _: TaskId,
        operation: u16,
        _: &[u8],
        response: &mut [u8],
        leases: &mut [Lease],
    ) -> Result<(ResponseCode, usize), TaskDeath> {
        if operation == 2 {
            assert!(leases.is_empty());
            let result: Result<bool, super::BoardError> = Ok(true);
            let length = hubpack::serialize(response, &result).unwrap();
            return Ok((ResponseCode::SUCCESS, length));
        }
        assert_eq!(leases.len(), 1);
        let expected = match operation {
            0 | 3 => 1,
            1 => 2,
            _ => panic!("unexpected operation"),
        };
        assert_eq!(leases[0].attributes, expected);
        let result: Result<(), super::BoardError> = Ok(());
        let length = hubpack::serialize(response, &result).unwrap();
        Ok((ResponseCode::SUCCESS, length))
    }
}

mod idyll_runtime {
    pub trait FromTaskDeath {
        fn from_task_death(death: super::userlib::TaskDeath) -> Self;
    }
    impl<T, E: FromTaskDeath> FromTaskDeath for Result<T, E> {
        fn from_task_death(death: super::userlib::TaskDeath) -> Self {
            Err(E::from_task_death(death))
        }
    }
}

#[test]
fn generated_calls_use_the_declared_lease_permissions_and_decode_replies() {
    let board = Node75Board::from(userlib::TaskId::gen0(1));
    assert_eq!(board.i2c_write(0xda, &[0x0d]), Ok(()));
    assert_eq!(board.i2c_read(0, &mut [0; 2]), Ok(()));
    assert_eq!(board.touch_ready(), Ok(true));
    assert_eq!(
        board.apply_writes(&[GpioWrite::new(Register::ClearB, 0, 1 << 14)]),
        Ok(()),
    );
}

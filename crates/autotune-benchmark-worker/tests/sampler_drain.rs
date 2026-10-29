use autotune_benchmark_worker::sampler::drain::{poll, DrainSource, Step};
struct Raced {
    done: bool,
    stage: u8,
}
impl DrainSource for Raced {
    type Item = u8;
    type Error = u32;
    fn inputs_finished(&self) -> bool {
        self.done
    }
    fn receive(&mut self) -> Result<Option<u8>, u32> {
        self.stage += 1;
        match self.stage {
            1 => {
                self.done = true;
                Ok(None)
            }
            2 => Ok(Some(7)),
            _ => Err(109),
        }
    }
}
#[test]
fn completion_must_be_observed_before_empty_receive_to_avoid_losing_a_raced_packet() {
    let mut source = Raced {
        done: false,
        stage: 0,
    };
    assert!(matches!(poll(&mut source), Step::Waiting));
    assert!(matches!(poll(&mut source), Step::Item(7)));
    assert!(matches!(poll(&mut source), Step::Finished(Some(109))));
}
#[test]
fn transport_error_does_not_mean_reader_completion() {
    struct Failed;
    impl DrainSource for Failed {
        type Item = u8;
        type Error = u32;
        fn inputs_finished(&self) -> bool {
            false
        }
        fn receive(&mut self) -> Result<Option<u8>, u32> {
            Err(5)
        }
    }
    assert!(matches!(poll(&mut Failed), Step::Failed(5)));
}

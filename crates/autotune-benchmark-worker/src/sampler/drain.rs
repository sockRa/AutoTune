//! Nonblocking drain: completion must precede the empty-queue observation.
//! Checking it afterwards can lose a packet enqueued between the two reads.
pub trait DrainSource {
    type Item;
    type Error;
    fn inputs_finished(&self) -> bool;
    fn receive(&mut self) -> Result<Option<Self::Item>, Self::Error>;
}
pub enum Step<T, E> {
    Item(T),
    Waiting,
    Failed(E),
    Finished(Option<E>),
}
pub fn poll<S: DrainSource>(source: &mut S) -> Step<S::Item, S::Error> {
    let finished = source.inputs_finished();
    match source.receive() {
        Ok(Some(item)) => Step::Item(item),
        Ok(None) if finished => Step::Finished(None),
        Err(error) if finished => Step::Finished(Some(error)),
        Ok(None) => Step::Waiting,
        Err(error) => Step::Failed(error),
    }
}

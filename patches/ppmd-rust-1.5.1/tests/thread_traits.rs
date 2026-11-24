use ppmd_rust::*;
use std::{
    cell::Cell,
    io::{Read, Write},
    marker::PhantomData,
    sync::MutexGuard,
};

// Cell is Send but not Sync; MutexGuard is Sync but not Send.
struct LocalIo<T>(PhantomData<T>);
impl<T> Read for LocalIo<T> {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}
impl<T> Write for LocalIo<T> {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Ok(0)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn require_send<T: Send>() {}
fn require_sync<T: Sync>() {}
#[test]
fn stream_traits_are_independent() {
    require_send::<Ppmd7Decoder<LocalIo<Cell<u8>>>>();
    require_sync::<Ppmd7Decoder<LocalIo<MutexGuard<'static, ()>>>>();
    require_send::<Ppmd7Decoder<std::io::Cursor<Vec<u8>>>>();
    require_sync::<Ppmd7Decoder<std::io::Cursor<Vec<u8>>>>();
    require_send::<Ppmd7aDecoder<LocalIo<Cell<u8>>>>();
    require_sync::<Ppmd7aDecoder<LocalIo<MutexGuard<'static, ()>>>>();
    require_send::<Ppmd7aDecoder<std::io::Cursor<Vec<u8>>>>();
    require_sync::<Ppmd7aDecoder<std::io::Cursor<Vec<u8>>>>();
    require_send::<Ppmd8Decoder<LocalIo<Cell<u8>>>>();
    require_sync::<Ppmd8Decoder<LocalIo<MutexGuard<'static, ()>>>>();
    require_send::<Ppmd8Decoder<std::io::Cursor<Vec<u8>>>>();
    require_sync::<Ppmd8Decoder<std::io::Cursor<Vec<u8>>>>();
    require_send::<Ppmd7Encoder<LocalIo<Cell<u8>>>>();
    require_sync::<Ppmd7Encoder<LocalIo<MutexGuard<'static, ()>>>>();
    require_send::<Ppmd7Encoder<std::io::Cursor<Vec<u8>>>>();
    require_sync::<Ppmd7Encoder<std::io::Cursor<Vec<u8>>>>();
    require_send::<Ppmd7aEncoder<LocalIo<Cell<u8>>>>();
    require_sync::<Ppmd7aEncoder<LocalIo<MutexGuard<'static, ()>>>>();
    require_send::<Ppmd7aEncoder<std::io::Cursor<Vec<u8>>>>();
    require_sync::<Ppmd7aEncoder<std::io::Cursor<Vec<u8>>>>();
    require_send::<Ppmd8Encoder<LocalIo<Cell<u8>>>>();
    require_sync::<Ppmd8Encoder<LocalIo<MutexGuard<'static, ()>>>>();
    require_send::<Ppmd8Encoder<std::io::Cursor<Vec<u8>>>>();
    require_sync::<Ppmd8Encoder<std::io::Cursor<Vec<u8>>>>();
}

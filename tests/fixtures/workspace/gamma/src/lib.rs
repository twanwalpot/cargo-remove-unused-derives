use serde::Deserialize;

// Trait bound with a generic lifetime arg: the diagnostic references
// `User: serde::Deserialize<'de>`, which the derive matcher must
// resolve to the bare `Deserialize` derive.
#[derive(Debug, Deserialize)]
pub struct User {
    pub id: String,
}

pub fn decode<'de, T: Deserialize<'de>>(_input: &'de str) -> Option<T> {
    None
}

pub fn use_user() {
    let _: Option<User> = decode("{}");
}

// Move-out / use-of-moved: the diagnostic spans point only at the use site.
// The tool has to recover the item via the `has type `Op`` label on the
// secondary span and pull `Copy` out of the same label.
#[derive(Debug, Clone, Copy)]
pub struct Op {
    pub n: u32,
}

pub fn use_op() -> (Op, Op) {
    let op = Op { n: 0 };
    (op, op)
}

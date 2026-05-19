#[derive(Debug, Clone)]
pub struct Beta {
    pub v: u32,
}

pub fn go() -> Beta {
    let b = Beta { v: 1 };
    println!("{:?}", b);
    b.clone()
}

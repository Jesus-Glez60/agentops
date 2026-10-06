pub struct Foo;

pub trait SomeTrait {
    fn forward(&self) -> i32;
}

impl Foo {
    pub fn forward(&self) -> i32 {
        1
    }
}

impl SomeTrait for Foo {
    fn forward(&self) -> i32 {
        2
    }
}

/// Lets `references` tests exercise a real, resolvable call-site for the
/// inherent `forward` (line 8) distinct from its own declaration.
pub fn call_inherent_forward(f: &Foo) -> i32 {
    f.forward()
}

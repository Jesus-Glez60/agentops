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

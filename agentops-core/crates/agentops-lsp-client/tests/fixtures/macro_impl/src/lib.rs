pub struct Foo;

macro_rules! impl_forward {
    ($t:ty) => {
        impl $t {
            pub fn generated_forward(&self) -> i32 {
                42
            }
        }
    };
}

impl_forward!(Foo);

#[derive(Debug)]
pub struct Bar;

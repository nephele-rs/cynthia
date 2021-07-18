#[macro_export]
macro_rules! pin {
    ($($x:ident),* $(,)?) => {
        $(
            let mut $x = $x;
            #[allow(unused_mut)]
            let mut $x = unsafe {
                core::pin::Pin::new_unchecked(&mut $x)
            };
        )*
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pixel {
    pub x: usize,
    pub y: usize,
}

impl Pixel {
    pub const fn new(x: usize, y: usize) -> Self {
        Self { x, y }
    }

    pub const fn to_coord(self) -> (usize, usize) {
        (self.x, self.y)
    }
}

#[macro_export]
macro_rules! pixel {
    ($x:expr, $y:expr) => {
        $crate::pixel::Pixel::new($x as usize, $y as usize)
    };
}

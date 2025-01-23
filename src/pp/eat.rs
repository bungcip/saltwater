/// basically copying from https://github.com/eddyb/cprep/

/// Eat from an iterator.
pub trait Eat: Iterator + Clone {
    fn try_eat<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        let mut speculative = self.clone();
        let r = f(&mut speculative)?;
        *self = speculative;
        Some(r)
    }

    fn eat_if(&mut self, f: impl FnOnce(&Self::Item) -> bool) -> Option<Self::Item> {
        self.try_eat(|this| this.next().filter(f))
    }

    fn eat<T>(&mut self, x: T) -> bool
    where
        Self::Item: PartialEq<T>,
    {
        self.eat_if(|y| *y == x).is_some()
    }

    fn eat_seq<T>(&mut self, needle: impl Iterator<Item = T>) -> bool
    where
        Self::Item: PartialEq<T>,
    {
        let mut speculative = self.clone();
        for x in needle {
            if speculative.next().map_or(true, |y| y != x) {
                return false;
            }
        }
        *self = speculative;
        true
    }
}

impl<I: Iterator + Clone> Eat for I {}

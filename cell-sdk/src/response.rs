// SPDX-License-Identifier: MIT

pub enum Response<'a, T> {
    Owned(Vec<u8>),
    Borrowed(&'a [u8]),
    Typed(T),
}

impl<'a, T> Response<'a, T> {
    pub fn into_owned(self) -> Vec<u8> {
        match self {
            Response::Owned(v) => v,
            Response::Borrowed(v) => v.to_vec(),
            _ => Vec::new(),
        }
    }
}

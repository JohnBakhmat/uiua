//! Algorithms for monadic array operations

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    f64::consts::{PI, TAU},
    iter::repeat,
    mem::size_of,
    ptr,
};

use ecow::{eco_vec, EcoVec};
use rayon::prelude::*;

use crate::{
    array::*,
    cowslice::{cowslice, CowSlice},
    grid_fmt::GridFmt,
    value::Value,
    Boxed, Shape, Uiua, UiuaResult,
};

use super::{op_bytes_retry_fill, ArrayCmpSlice, FillContext};

impl Value {
    /// Make the value 1-dimensional
    pub fn deshape(&mut self) {
        self.deshape_depth(0);
    }
    /// Add a 1-length dimension to the front of the value's shape
    pub fn fix(&mut self) {
        self.shape_mut().insert(0, 1);
    }
    pub(crate) fn inv_fix(&mut self) {
        let shape = self.shape_mut();
        if shape.starts_with(&[1]) {
            shape.remove(0);
        } else if shape.len() >= 2 {
            let new_first_dim = shape[0] * shape[1];
            shape.drain(0..2);
            shape.insert(0, new_first_dim);
        }
    }
    pub(crate) fn deshape_depth(&mut self, depth: usize) {
        match self {
            Value::Num(n) => n.deshape_depth(depth),
            #[cfg(feature = "bytes")]
            Value::Byte(b) => b.deshape_depth(depth),
            Value::Complex(c) => c.deshape_depth(depth),
            Value::Char(c) => c.deshape_depth(depth),
            Value::Box(b) => {
                if let Some(bx) = b.as_scalar_mut() {
                    bx.as_value_mut().deshape_depth(depth);
                } else if depth > 0 && b.rank() <= 1 {
                    for b in b.data.as_mut_slice() {
                        b.as_value_mut().deshape_depth(depth - 1);
                    }
                } else {
                    b.deshape_depth(depth);
                }
            }
        }
    }
    /// Attempt to parse the value into a number
    pub fn parse_num(&self, env: &Uiua) -> UiuaResult<Self> {
        Ok(match (self, self.shape().sizes()) {
            (Value::Char(arr), [] | [_]) => {
                let mut s: String = arr.data.iter().copied().collect();
                if s.contains('¯') {
                    s = s.replace('¯', "-");
                }
                if s.contains('`') {
                    s = s.replace('`', "-");
                }
                if s.contains('η') {
                    s = s.replace('η', &(PI * 0.5).to_string());
                }
                if s.contains('π') {
                    s = s.replace('π', &PI.to_string());
                }
                if s.contains('τ') {
                    s = s.replace('τ', &TAU.to_string());
                }
                if s.contains('∞') {
                    s = s.replace('∞', &f64::INFINITY.to_string());
                }
                s.parse::<f64>()
                    .map_err(|e| env.error(format!("Cannot parse into number: {}", e)))?
                    .into()
            }
            (Value::Box(arr), []) => {
                let value = &arr.data[0].0;
                value.parse_num(env)?
            }
            (Value::Char(_) | Value::Box(_), _) => {
                let mut rows = Vec::with_capacity(self.row_count());
                for row in self.rows() {
                    rows.push(row.parse_num(env)?);
                }
                Value::from_row_values(rows, env)?
            }
            (val, _) => return Err(env.error(format!("Cannot parse {} array", val.type_name()))),
        })
    }
    pub(crate) fn inv_parse(&self, env: &Uiua) -> UiuaResult<Self> {
        if self.rank() == 0 {
            return match self {
                Value::Box(b) => b.as_scalar().unwrap().as_value().inv_parse(env),
                value => Ok(value.show().into()),
            };
        }
        Ok(match self {
            Value::Num(nums) => {
                let new_data: CowSlice<Boxed> = (nums.data.iter().map(GridFmt::grid_string))
                    .map(Value::from)
                    .map(Boxed)
                    .collect();
                Array::new(nums.shape.clone(), new_data).into()
            }
            #[cfg(feature = "bytes")]
            Value::Byte(bytes) => {
                let new_data: CowSlice<Boxed> = (bytes.data.iter().map(GridFmt::grid_string))
                    .map(Value::from)
                    .map(Boxed)
                    .collect();
                Array::new(bytes.shape.clone(), new_data).into()
            }
            Value::Complex(complexes) => {
                let new_data: CowSlice<Boxed> = (complexes.data.iter().map(GridFmt::grid_string))
                    .map(Value::from)
                    .map(Boxed)
                    .collect();
                Array::new(complexes.shape.clone(), new_data).into()
            }
            val => return Err(env.error(format!("Cannot invert parse {} array", val.type_name()))),
        })
    }
}

impl<T: ArrayValue> Array<T> {
    /// Make the array 1-dimensional
    pub fn deshape(&mut self) {
        self.shape = self.element_count().into();
    }
    pub(crate) fn deshape_depth(&mut self, mut depth: usize) {
        depth = depth.min(self.rank());
        let deshaped: usize = self
            .shape
            .split_off(depth)
            .into_iter()
            .map(|dim| dim.size)
            .product();
        self.shape.push(deshaped);
    }
}

impl Value {
    /// Create a `range` array
    pub fn range(&self, env: &Uiua) -> UiuaResult<Self> {
        let shape = &self.as_nats(
            env,
            "Range max should be a single natural number \
            or a list of natural numbers",
        )?;
        if self.rank() == 0 {
            return Ok((0..shape[0]).collect());
        }
        if shape.is_empty() {
            return Ok(Array::<f64>::new(0, CowSlice::new()).into());
        }
        let mut shape = Shape::from(shape.as_slice());
        let data = range(&shape, env)?;
        shape.push(shape.len());
        Ok(Array::new(shape, data).into())
    }
}

fn range(shape: &[usize], env: &Uiua) -> UiuaResult<CowSlice<f64>> {
    if shape.is_empty() {
        return Ok(cowslice![0.0]);
    }
    if shape.contains(&0) {
        return Ok(CowSlice::new());
    }
    let mut len = shape.len();
    for &item in shape {
        let (new, overflow) = len.overflowing_mul(item);
        if overflow || new > 2usize.pow(30) / size_of::<f64>() {
            let len = shape.len() as f64 * shape.iter().map(|d| *d as f64).product::<f64>();
            return Err(env.error(format!(
                "Attempting to make a range from shape {} would \
                create an array with {} elements, which is too large",
                FormatShape(shape),
                len
            )));
        }
        len = new;
    }
    let mut data: EcoVec<f64> = eco_vec![0.0; len];
    let data_slice = data.make_mut();
    let mut curr = vec![0; shape.len()];
    let mut i = 0;
    loop {
        for d in &curr {
            data_slice[i] = *d as f64;
            i += 1;
        }
        let mut j = shape.len() - 1;
        loop {
            curr[j] += 1;
            if curr[j] == shape[j] {
                curr[j] = 0;
                if j == 0 {
                    return Ok(data.into());
                }
                j -= 1;
            } else {
                break;
            }
        }
    }
}

impl Value {
    /// Get the first row of the value
    pub fn first(self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_into_deep(
            |a| a.first(env).map(Into::into),
            |a| {
                op_bytes_retry_fill(
                    a,
                    |a| a.first(env).map(Into::into),
                    |a| a.first(env).map(Into::into),
                )
            },
            |a| a.first(env).map(Into::into),
            |a| a.first(env).map(Into::into),
            |a| a.first(env).map(Into::into),
        )
    }
    /// Get the last row of the value
    pub fn last(self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_into_deep(
            |a| a.last(env).map(Into::into),
            |a| {
                op_bytes_retry_fill(
                    a,
                    |a| a.last(env).map(Into::into),
                    |a| a.last(env).map(Into::into),
                )
            },
            |a| a.last(env).map(Into::into),
            |a| a.last(env).map(Into::into),
            |a| a.last(env).map(Into::into),
        )
    }
    pub(crate) fn unfirst(self, into: Self, env: &Uiua) -> UiuaResult<Self> {
        into.try_map_boxed(|into| {
            self.generic_bin_into(
                into.unboxed(),
                |a, b| a.unfirst(b, env).map(Into::into),
                |a, b| a.unfirst(b, env).map(Into::into),
                |a, b| a.unfirst(b, env).map(Into::into),
                |a, b| a.unfirst(b, env).map(Into::into),
                |a, b| a.unfirst(b, env).map(Into::into),
                |a, b| {
                    env.error(format!(
                        "Cannot invert first of {} into {}",
                        a.type_name(),
                        b.type_name()
                    ))
                },
            )
        })
    }
    pub(crate) fn unlast(self, into: Self, env: &Uiua) -> UiuaResult<Self> {
        into.try_map_boxed(|into| {
            self.generic_bin_into(
                into.unboxed(),
                |a, b| a.unlast(b, env).map(Into::into),
                |a, b| a.unlast(b, env).map(Into::into),
                |a, b| a.unlast(b, env).map(Into::into),
                |a, b| a.unlast(b, env).map(Into::into),
                |a, b| a.unlast(b, env).map(Into::into),
                |a, b| {
                    env.error(format!(
                        "Cannot invert last of {} into {}",
                        a.type_name(),
                        b.type_name()
                    ))
                },
            )
        })
    }
}

impl<T: ArrayValue> Array<T> {
    /// Get the first row of the array
    pub fn first(mut self, env: &Uiua) -> UiuaResult<Self> {
        match &*self.shape {
            [] => Err(env.error("Cannot take first of a scalar")),
            [0, rest @ ..] => match env.fill() {
                Ok(fill) => {
                    self.data.extend(repeat(fill).take(self.row_len()));
                    self.shape = rest.into();
                    Ok(self)
                }
                Err(e) => Err(env
                    .error(format!("Cannot take first of an empty array{e}"))
                    .fill()),
            },
            _ => {
                let row_len = self.row_len();
                self.shape.remove(0);
                self.data.truncate(row_len);
                Ok(self)
            }
        }
    }
    /// Get the last row of the array
    pub fn last(mut self, env: &Uiua) -> UiuaResult<Self> {
        match &*self.shape {
            [] => Err(env.error("Cannot take last of a scalar")),
            [0, rest @ ..] => match env.fill() {
                Ok(fill) => {
                    self.data.extend(repeat(fill).take(self.row_len()));
                    self.shape = rest.into();
                    Ok(self)
                }
                Err(e) => Err(env
                    .error(format!("Cannot take last of an empty array{e}"))
                    .fill()),
            },
            _ => {
                let row_len = self.row_len();
                self.shape.remove(0);
                let prefix_len = self.data.len() - row_len;
                self.data = self.data.into_iter().skip(prefix_len).collect();
                Ok(self)
            }
        }
    }
    pub(crate) fn unfirst(self, into: Self, env: &Uiua) -> UiuaResult<Self> {
        self.join(into.drop(&[1], env)?, env)
    }
    pub(crate) fn unlast(self, into: Self, env: &Uiua) -> UiuaResult<Self> {
        into.drop(&[-1], env)?.join(self, env)
    }
}

impl Value {
    /// Reverse the rows of the value
    pub fn reverse(&mut self) {
        self.reverse_depth(0);
    }
    pub(crate) fn reverse_depth(&mut self, depth: usize) {
        match self {
            Value::Num(n) => n.reverse_depth(depth),
            #[cfg(feature = "bytes")]
            Value::Byte(b) => b.reverse_depth(depth),
            Value::Complex(c) => c.reverse_depth(depth),
            Value::Char(c) => c.reverse_depth(depth),
            Value::Box(b) => {
                if let Some(bx) = b.as_scalar_mut() {
                    bx.as_value_mut().reverse_depth(depth);
                } else if depth > 0 && b.rank() <= 1 {
                    for b in b.data.as_mut_slice() {
                        b.as_value_mut().reverse_depth(depth - 1);
                    }
                } else {
                    b.reverse_depth(depth);
                }
            }
        }
    }
}

impl<T: ArrayValue> Array<T> {
    /// Reverse the rows of the array
    pub fn reverse(&mut self) {
        self.reverse_depth(0);
    }
    pub(crate) fn reverse_depth(&mut self, mut depth: usize) {
        depth = depth.min(self.rank());
        let row_shape = &self.shape[depth..];
        if row_shape.is_empty() {
            return;
        }
        let chunk_size = row_shape.iter().product();
        if chunk_size == 0 {
            return;
        }
        let data = self.data.as_mut_slice();
        let chunk_row_count = self.shape[depth];
        let chunk_row_len = chunk_size / chunk_row_count;
        for data in data.chunks_exact_mut(chunk_size) {
            for i in 0..chunk_row_count / 2 {
                let left = i * chunk_row_len;
                let right = (chunk_row_count - i - 1) * chunk_row_len;
                let left = &mut data[left] as *mut T;
                let right = &mut data[right] as *mut T;
                unsafe {
                    ptr::swap_nonoverlapping(left, right, chunk_row_len);
                }
            }
        }
    }
}

impl Value {
    /// Transpose the value
    pub fn transpose(&mut self) {
        self.generic_mut_deep(
            Array::transpose,
            Array::transpose,
            Array::transpose,
            Array::transpose,
            Array::transpose,
        )
    }
    pub(crate) fn transpose_depth(&mut self, depth: usize, amnt: i32) {
        match self {
            Value::Num(n) => n.transpose_depth(depth, amnt),
            #[cfg(feature = "bytes")]
            Value::Byte(b) => b.transpose_depth(depth, amnt),
            Value::Complex(c) => c.transpose_depth(depth, amnt),
            Value::Char(c) => c.transpose_depth(depth, amnt),
            Value::Box(b) => {
                if let Some(bx) = b.as_scalar_mut() {
                    bx.as_value_mut().transpose_depth(depth, amnt);
                } else if depth > 0 && b.rank() <= 1 {
                    for b in b.data.as_mut_slice() {
                        b.as_value_mut().transpose_depth(depth - 1, amnt);
                    }
                } else {
                    b.transpose_depth(depth, amnt);
                }
            }
        }
    }
}

impl<T: ArrayValue> Array<T> {
    /// Transpose the array
    pub fn transpose(&mut self) {
        self.transpose_depth(0, 1);
    }
    pub(crate) fn transpose_depth(&mut self, mut depth: usize, amnt: i32) {
        crate::profile_function!();
        depth = depth.min(self.rank());
        let count = amnt.unsigned_abs() as usize;
        if self.rank() - depth < 2 || depth + count == self.rank() || count == 0 {
            return;
        }
        let forward = amnt.is_positive();
        if self.shape[depth..].iter().any(|&d| d == 0) {
            if forward {
                self.shape[depth..].rotate_left(count);
            } else {
                self.shape[depth..].rotate_right(count);
            }
            return;
        }
        let data_slice = self.data.as_mut_slice();
        for data in data_slice.chunks_exact_mut(self.shape[depth..].iter().product()) {
            let mut temp = data.to_vec();
            let subs: usize = if forward {
                self.shape[depth..].iter().take(count).product()
            } else {
                self.shape[depth..].iter().rev().skip(count).product()
            };
            let stride = data.len() / subs;
            let op = |(temp_chunk_i, chunk): (usize, &mut [T])| {
                for (chunk_i, item) in chunk.iter_mut().enumerate() {
                    *item = data[chunk_i * stride + temp_chunk_i].clone();
                }
            };
            if subs > 500 {
                temp.par_chunks_mut(subs).enumerate().for_each(op);
            } else {
                temp.chunks_mut(subs).enumerate().for_each(op);
            }
            data.clone_from_slice(&temp);
        }
        if forward {
            self.shape[depth..].rotate_left(count);
        } else {
            self.shape[depth..].rotate_right(count);
        }
    }
}

impl Value {
    /// Get the `rise` of the value
    pub fn rise(&self, env: &Uiua) -> UiuaResult<Vec<usize>> {
        self.generic_ref_env_deep(
            Array::rise,
            Array::rise,
            Array::rise,
            Array::rise,
            Array::rise,
            env,
        )
    }
    /// Get the `fall` of the value
    pub fn fall(&self, env: &Uiua) -> UiuaResult<Vec<usize>> {
        self.generic_ref_env_deep(
            Array::fall,
            Array::fall,
            Array::fall,
            Array::fall,
            Array::fall,
            env,
        )
    }
    /// Sort the value ascending
    pub fn sort_up(&mut self, env: &Uiua) -> UiuaResult {
        self.generic_mut_deep(
            |a| a.sort_up(env),
            |a| a.sort_up(env),
            |a| a.sort_up(env),
            |a| a.sort_up(env),
            |a| a.sort_up(env),
        )
    }
    /// Sort the value descending
    pub fn sort_down(&mut self, env: &Uiua) -> UiuaResult {
        self.generic_mut_deep(
            |a| a.sort_down(env),
            |a| a.sort_down(env),
            |a| a.sort_down(env),
            |a| a.sort_down(env),
            |a| a.sort_down(env),
        )
    }
    /// `classify` the rows of the value
    pub fn classify(&self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_ref_env_deep(
            Array::classify,
            Array::classify,
            Array::classify,
            Array::classify,
            Array::classify,
            env,
        )
        .map(Self::from_iter)
    }
    /// `deduplicate` the rows of the value
    pub fn deduplicate(&mut self) {
        self.generic_mut_deep(
            Array::deduplicate,
            Array::deduplicate,
            Array::deduplicate,
            Array::deduplicate,
            Array::deduplicate,
        )
    }
}

impl<T: ArrayValue> Array<T> {
    /// Get the `rise` of the array
    pub fn rise(&self, env: &Uiua) -> UiuaResult<Vec<usize>> {
        if self.rank() == 0 {
            return Err(env.error("Cannot rise a scalar"));
        }
        if self.element_count() == 0 {
            return Ok(Vec::new());
        }
        let mut indices = (0..self.row_count()).collect::<Vec<_>>();
        indices.par_sort_by(|&a, &b| {
            self.row_slice(a)
                .iter()
                .zip(self.row_slice(b))
                .map(|(a, b)| a.array_cmp(b))
                .find(|x| x != &Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        });
        Ok(indices)
    }
    /// Get the `fall` of the array
    pub fn fall(&self, env: &Uiua) -> UiuaResult<Vec<usize>> {
        if self.rank() == 0 {
            return Err(env.error("Cannot fall a scalar"));
        }
        if self.element_count() == 0 {
            return Ok(Vec::new());
        }
        let mut indices = (0..self.row_count()).collect::<Vec<_>>();
        indices.par_sort_by(|&a, &b| {
            self.row_slice(a)
                .iter()
                .zip(self.row_slice(b))
                .map(|(a, b)| b.array_cmp(a))
                .find(|x| x != &Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        });
        Ok(indices)
    }
    /// Sort an array ascending
    pub fn sort_up(&mut self, env: &Uiua) -> UiuaResult {
        if self.rank() == 0 {
            return Err(env.error("Cannot rise a scalar"));
        }
        if self.element_count() == 0 {
            return Ok(());
        }
        if self.rank() == 1 {
            self.data.as_mut_slice().par_sort_by(|a, b| a.array_cmp(b));
        } else {
            let rise = self.rise(env)?;
            let mut new_data = EcoVec::with_capacity(self.data.len());
            for i in rise {
                new_data.extend_from_slice(self.row_slice(i));
            }
            self.data = new_data.into();
        }
        Ok(())
    }
    /// Sort an array descending
    pub fn sort_down(&mut self, env: &Uiua) -> UiuaResult {
        if self.rank() == 0 {
            return Err(env.error("Cannot fall a scalar"));
        }
        if self.element_count() == 0 {
            return Ok(());
        }
        if self.rank() == 1 {
            self.data.as_mut_slice().par_sort_by(|a, b| b.array_cmp(a));
        } else {
            let fall = self.fall(env)?;
            let mut new_data = EcoVec::with_capacity(self.data.len());
            for i in fall {
                new_data.extend_from_slice(self.row_slice(i));
            }
            self.data = new_data.into();
        }
        Ok(())
    }
    /// `classify` the rows of the array
    pub fn classify(&self, env: &Uiua) -> UiuaResult<Vec<usize>> {
        if self.rank() == 0 {
            return Err(env.error("Cannot classify a rank-0 array"));
        }
        let mut classes = HashMap::new();
        let mut classified = Vec::with_capacity(self.row_count());
        for row in self.row_slices() {
            let new_class = classes.len();
            let class = *classes.entry(ArrayCmpSlice(row)).or_insert(new_class);
            classified.push(class);
        }
        Ok(classified)
    }
    /// `deduplicate` the rows of the array
    pub fn deduplicate(&mut self) {
        if self.rank() == 0 {
            return;
        }
        let mut deduped = CowSlice::new();
        let mut seen = HashSet::new();
        let mut new_len = 0;
        for row in self.row_slices() {
            if seen.insert(ArrayCmpSlice(row)) {
                deduped.extend_from_slice(row);
                new_len += 1;
            }
        }
        self.data = deduped;
        self.shape[0] = new_len;
    }
}

impl Value {
    /// Encode the `bits` of the value
    pub fn bits(&self, env: &Uiua) -> UiuaResult<Array<u8>> {
        match self {
            #[cfg(feature = "bytes")]
            Value::Byte(n) => n.convert_ref().bits(env),
            Value::Num(n) => n.bits(env),
            _ => Err(env.error("Argument to bits must be an array of natural numbers")),
        }
    }
    /// Decode the `bits` of the value
    pub fn inv_bits(&self, env: &Uiua) -> UiuaResult<Array<f64>> {
        match self {
            #[cfg(feature = "bytes")]
            Value::Byte(n) => n.inverse_bits(env),
            Value::Num(n) => n.convert_ref_with(|n| n as u8).inverse_bits(env),
            _ => Err(env.error("Argument to inverse_bits must be an array of naturals")),
        }
    }
}

impl Array<f64> {
    /// Encode the `bits` of the array
    pub fn bits(&self, env: &Uiua) -> UiuaResult<Array<u8>> {
        let mut nats = Vec::new();
        for &n in &self.data {
            if n.fract() != 0.0 {
                return Err(env.error("Array must be a list of naturals"));
            }
            nats.push(n as u128);
        }
        let mut max = if let Some(max) = nats.iter().max() {
            *max
        } else {
            let mut shape = self.shape.clone();
            shape.push(0);
            return Ok(Array::new(shape, CowSlice::new()));
        };
        let mut max_bits = 0;
        while max != 0 {
            max_bits += 1;
            max >>= 1;
        }
        let mut new_data = EcoVec::from_elem(0, self.data.len() * max_bits);
        let new_data_slice = new_data.make_mut();
        // Big endian
        for (i, n) in nats.into_iter().enumerate() {
            for j in 0..max_bits {
                let index = i * max_bits + j;
                new_data_slice[index] = u8::from(n & (1 << j) != 0);
            }
        }
        let mut shape = self.shape.clone();
        shape.push(max_bits);
        let mut arr = Array::new(shape, new_data);
        arr.validate_shape();
        arr.meta_mut().flags.set(ArrayFlags::BOOLEAN, true);
        Ok(arr)
    }
}

impl Array<u8> {
    /// Decode the `bits` of the array
    pub fn inverse_bits(&self, env: &Uiua) -> UiuaResult<Array<f64>> {
        let mut bools = Vec::with_capacity(self.data.len());
        for &b in &self.data {
            if b > 1 {
                return Err(env.error("Array must be a list of booleans"));
            }
            bools.push(b != 0);
        }
        if bools.is_empty() {
            if self.shape.is_empty() {
                return Ok(Array::from(0.0));
            }
            let mut shape = self.shape.clone();
            shape.pop();
            let count: usize = shape.iter().product();
            return Ok(Array::new(shape, cowslice![0.0; count]));
        }
        if self.rank() == 0 {
            return Ok(Array::from(bools[0] as u8 as f64));
        }
        let mut shape = self.shape.clone();
        let bit_string_len = shape.pop().unwrap();
        let mut new_data = EcoVec::from_elem(0.0, self.data.len() / bit_string_len);
        let new_data_slice = new_data.make_mut();
        // Big endian
        for (i, bits) in bools.chunks_exact(bit_string_len).enumerate() {
            let mut n: u128 = 0;
            for (j, b) in bits.iter().enumerate() {
                if *b {
                    n |= 1u128.overflowing_shl(j as u32).0;
                }
            }
            new_data_slice[i] = n as f64;
        }
        Ok(Array::new(shape, new_data))
    }
}

impl Value {
    /// Get the indices `where` the value is nonzero
    pub fn wher(&self, env: &Uiua) -> UiuaResult<Array<f64>> {
        Ok(if self.rank() <= 1 {
            let counts = self.as_nats(env, "Argument to where must be an array of naturals")?;
            let total: usize = counts.iter().fold(0, |acc, &b| acc.saturating_add(b));
            let mut data = EcoVec::with_capacity(total);
            for (i, &b) in counts.iter().enumerate() {
                for _ in 0..b {
                    let i = i as f64;
                    data.push(i);
                }
            }
            Array::from(data)
        } else {
            let counts =
                self.as_natural_array(env, "Argument to where must be an array of naturals")?;
            let total: usize = counts.data.iter().fold(0, |acc, &b| acc.saturating_add(b));
            let mut data = EcoVec::with_capacity(total);
            for (i, &b) in counts.data.iter().enumerate() {
                for _ in 0..b {
                    let mut i = i;
                    let start = data.len();
                    for &d in counts.shape.iter().rev() {
                        data.insert(start, (i % d) as f64);
                        i /= d;
                    }
                }
            }
            let shape = Shape::from([total, counts.rank()].as_ref());
            Array::new(shape, data)
        })
    }
    /// Get the `first` index `where` the value is nonzero
    pub fn first_where(&self, env: &Uiua) -> UiuaResult<Array<f64>> {
        if self.rank() <= 1 {
            match self {
                Value::Num(nums) => {
                    for (i, n) in nums.data.iter().enumerate() {
                        if n.fract() != 0.0 || *n < 0.0 {
                            return Err(env.error("Argument to where must be an array of naturals"));
                        }
                        if *n != 0.0 {
                            return Ok(Array::scalar(i as f64));
                        }
                    }
                    env.fill::<f64>()
                        .map(Array::scalar)
                        .map_err(|e| env.error(format!("Cannot take first of an empty array{e}")))
                }
                #[cfg(feature = "bytes")]
                Value::Byte(bytes) => {
                    for (i, n) in bytes.data.iter().enumerate() {
                        if *n != 0 {
                            return Ok(Array::scalar(i as f64));
                        }
                    }
                    env.fill::<f64>()
                        .map(Array::scalar)
                        .map_err(|e| env.error(format!("Cannot take first of an empty array{e}")))
                }
                value => Err(env.error(format!(
                    "Argument to where must be an array of naturals, but it is {}",
                    value.type_name_plural()
                ))),
            }
        } else {
            match self {
                Value::Num(nums) => {
                    for (i, n) in nums.data.iter().enumerate() {
                        if n.fract() != 0.0 || *n < 0.0 {
                            return Err(env.error("Argument to where must be an array of naturals"));
                        }
                        if *n != 0.0 {
                            let mut i = i;
                            let mut res = Vec::with_capacity(nums.rank());
                            for &d in nums.shape.iter().rev() {
                                res.insert(0, (i % d) as f64);
                                i /= d;
                            }
                            return Ok(Array::from_iter(res));
                        }
                    }
                    env.fill::<f64>()
                        .map(Array::scalar)
                        .map_err(|e| env.error(format!("Cannot take first of an empty array{e}")))
                }
                #[cfg(feature = "bytes")]
                Value::Byte(bytes) => {
                    for (i, n) in bytes.data.iter().enumerate() {
                        if *n != 0 {
                            let mut i = i;
                            let mut res = Vec::with_capacity(bytes.rank());
                            for &d in bytes.shape.iter().rev() {
                                res.insert(0, (i % d) as f64);
                                i /= d;
                            }
                            return Ok(Array::from_iter(res));
                        }
                    }
                    env.fill::<f64>()
                        .map(Array::scalar)
                        .map_err(|e| env.error(format!("Cannot take first of an empty array{e}")))
                }
                value => Err(env.error(format!(
                    "Argument to where must be an array of naturals, but it is {}",
                    value.type_name_plural()
                ))),
            }
        }
    }
    /// `invert` `where`
    pub fn inverse_where(&self, env: &Uiua) -> UiuaResult<Self> {
        Ok(match self.shape().sizes() {
            [] | [_] => {
                let indices =
                    self.as_nats(env, "Argument to inverse where must be a list of naturals")?;
                let is_sorted = indices
                    .iter()
                    .zip(indices.iter().skip(1))
                    .all(|(&a, &b)| a <= b);
                let size = indices.iter().max().map(|&i| i + 1).unwrap_or(0);
                let mut data = EcoVec::with_capacity(size);
                if is_sorted {
                    let mut j = 0;
                    data.extend((0..size).map(|i| {
                        while indices.get(j).is_some_and(|&n| n < i) {
                            j += 1;
                        }
                        let mut count: usize = 0;
                        while indices.get(j).copied() == Some(i) {
                            j += 1;
                            count += 1;
                        }
                        count as f64
                    }));
                } else {
                    let mut counts = HashMap::new();
                    for &i in &indices {
                        *counts.entry(i).or_insert(0) += 1;
                    }
                    data.extend((0..size).map(|i| counts.get(&i).copied().unwrap_or(0) as f64));
                }
                Array::from(data).into()
            }
            [_, trailing] => {
                let indices = self.as_natural_array(
                    env,
                    "Argument to inverse where must be an array of naturals",
                )?;
                let mut counts: HashMap<&[usize], usize> = HashMap::new();
                for row in indices.row_slices() {
                    *counts.entry(row).or_default() += 1;
                }
                let mut init = Shape::with_capacity(*trailing);
                for _ in 0..*trailing {
                    init.push(0);
                }
                let shape = counts.keys().fold(init, |mut acc, row| {
                    for (a, r) in acc.iter_mut().zip(row.iter()) {
                        *a = (*a).max(*r + 1);
                    }
                    acc
                });
                let data_len: usize = shape.iter().product();
                let mut data = EcoVec::with_capacity(data_len);
                data.extend(repeat(0.0).take(data_len));
                let data_slice = data.make_mut();
                for (key, count) in counts {
                    let mut i = 0;
                    let mut row_len = 1;
                    for (d, &n) in shape.iter().zip(key).rev() {
                        i += n * row_len;
                        row_len *= d;
                    }
                    data_slice[i] = count as f64;
                }
                Array::new(shape, data).into()
            }
            shape => {
                return Err(env.error(format!("Cannot invert where of rank-{} array", shape.len())))
            }
        })
    }
}

impl Value {
    /// Convert a string value to a list of UTF-8 bytes
    pub fn utf8(&self, env: &Uiua) -> UiuaResult<Self> {
        let s = self.as_string(env, "Argument to utf must be a string")?;
        Ok(Array::<u8>::from_iter(s.into_bytes()).into())
    }
    /// Convert a list of UTF-8 bytes to a string value
    pub fn inv_utf8(&self, env: &Uiua) -> UiuaResult<Self> {
        let bytes = self.as_bytes(env, "Argument to inverse utf must be a list of bytes")?;
        let s = String::from_utf8(bytes).map_err(|e| env.error(e))?;
        Ok(s.into())
    }
}

impl Value {
    pub(crate) fn first_min_index(&self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_ref_env_deep(
            Array::first_min_index,
            Array::first_min_index,
            Array::first_min_index,
            Array::first_min_index,
            Array::first_min_index,
            env,
        )
        .map(Into::into)
    }
    pub(crate) fn first_max_index(&self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_ref_env_deep(
            Array::first_max_index,
            Array::first_max_index,
            Array::first_max_index,
            Array::first_max_index,
            Array::first_max_index,
            env,
        )
        .map(Into::into)
    }
    pub(crate) fn last_min_index(&self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_ref_env_deep(
            Array::last_min_index,
            Array::last_min_index,
            Array::last_min_index,
            Array::last_min_index,
            Array::last_min_index,
            env,
        )
        .map(Into::into)
    }
    pub(crate) fn last_max_index(&self, env: &Uiua) -> UiuaResult<Self> {
        self.generic_ref_env_deep(
            Array::last_max_index,
            Array::last_max_index,
            Array::last_max_index,
            Array::last_max_index,
            Array::last_max_index,
            env,
        )
        .map(Into::into)
    }
}

impl<T: ArrayValue> Array<T> {
    pub(crate) fn first_min_index(&self, env: &Uiua) -> UiuaResult<f64> {
        if self.rank() == 0 {
            return Err(env.error("Cannot get min index of a scalar"));
        }
        if self.row_count() == 0 {
            return env
                .fill::<f64>()
                .map_err(|e| env.error(format!("Cannot get min index of an empty array{e}")));
        }
        let index = self
            .row_slices()
            .map(ArrayCmpSlice)
            .enumerate()
            .min_by(|(_, a), (_, b)| a.cmp(b))
            .unwrap()
            .0;
        Ok(index as f64)
    }
    pub(crate) fn first_max_index(&self, env: &Uiua) -> UiuaResult<f64> {
        if self.rank() == 0 {
            return Err(env.error("Cannot get max index of a scalar"));
        }
        if self.row_count() == 0 {
            return env
                .fill::<f64>()
                .map_err(|e| env.error(format!("Cannot get max index of an empty array{e}")));
        }
        let index = self
            .row_slices()
            .map(ArrayCmpSlice)
            .enumerate()
            .min_by(|(_, a), (_, b)| a.cmp(b).reverse())
            .unwrap()
            .0;
        Ok(index as f64)
    }
    pub(crate) fn last_min_index(&self, env: &Uiua) -> UiuaResult<f64> {
        if self.rank() == 0 {
            return Err(env.error("Cannot get min index of a scalar"));
        }
        if self.row_count() == 0 {
            return env
                .fill::<f64>()
                .map_err(|e| env.error(format!("Cannot get min index of an empty array{e}")));
        }
        let index = self
            .row_slices()
            .map(ArrayCmpSlice)
            .enumerate()
            .max_by(|(_, a), (_, b)| a.cmp(b).reverse())
            .unwrap()
            .0;
        Ok(index as f64)
    }
    pub(crate) fn last_max_index(&self, env: &Uiua) -> UiuaResult<f64> {
        if self.rank() == 0 {
            return Err(env.error("Cannot get max index of a scalar"));
        }
        if self.row_count() == 0 {
            return env
                .fill::<f64>()
                .map_err(|e| env.error(format!("Cannot get max index of an empty array{e}")));
        }
        let index = self
            .row_slices()
            .map(ArrayCmpSlice)
            .enumerate()
            .max_by(|(_, a), (_, b)| a.cmp(b))
            .unwrap()
            .0;
        Ok(index as f64)
    }
}

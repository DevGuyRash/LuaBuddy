const CALL_DESCRIPTOR: &[u8] = b"(Lse/krka/kahlua/vm/LuaCallFrame;I)I";

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8], String> {
        let end = self
            .position
            .checked_add(size)
            .ok_or("classfile offset overflow")?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or("truncated classfile")?;
        self.position = end;
        Ok(result)
    }

    fn u1(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u2(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }
    fn u4(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn skip_attributes(&mut self) -> Result<(), String> {
        let count = self.u2()?;
        for _ in 0..count {
            self.u2()?;
            let length = self.u4()? as usize;
            self.take(length)?;
        }
        Ok(())
    }
}

/// Replaces only the existing, verified no-op callable body with a native declaration.
/// No instructions, helper classes, or application bytecode are inserted.
pub(crate) fn native_callable(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = Reader { bytes, position: 0 };
    if reader.u4()? != 0xcafe_babe {
        return Err("invalid classfile magic".to_owned());
    }
    reader.take(4)?;
    let pool_count = reader.u2()? as usize;
    let mut utf8 = vec![None; pool_count];
    let mut index = 1;
    while index < pool_count {
        match reader.u1()? {
            1 => {
                let length = reader.u2()? as usize;
                utf8[index] = Some(reader.take(length)?);
            }
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => {
                reader.take(4)?;
            }
            5 | 6 => {
                reader.take(8)?;
                index += 1;
            }
            7 | 8 | 16 | 19 | 20 => {
                reader.take(2)?;
            }
            15 => {
                reader.take(3)?;
            }
            tag => return Err(format!("unsupported classfile constant tag {tag}")),
        }
        index += 1;
    }
    reader.take(6)?;
    let interfaces = reader.u2()? as usize;
    reader.take(interfaces * 2)?;
    let fields = reader.u2()?;
    for _ in 0..fields {
        reader.take(6)?;
        reader.skip_attributes()?;
    }
    let methods = reader.u2()?;
    let mut output = Vec::with_capacity(bytes.len());
    let mut copied = 0;
    let mut changed = false;
    for _ in 0..methods {
        let method_start = reader.position;
        let flags = reader.u2()?;
        let name = reader.u2()? as usize;
        let descriptor = reader.u2()? as usize;
        let attributes = reader.u2()?;
        let target = utf8.get(name).copied().flatten() == Some(b"call".as_slice())
            && utf8.get(descriptor).copied().flatten() == Some(CALL_DESCRIPTOR);
        let mut retained = Vec::new();
        let mut code = None;
        for _ in 0..attributes {
            let start = reader.position;
            let attribute_name = reader.u2()? as usize;
            let length = reader.u4()? as usize;
            let body = reader.take(length)?;
            if target && utf8.get(attribute_name).copied().flatten() == Some(b"Code".as_slice()) {
                code = Some(body);
            } else {
                retained.push(&bytes[start..reader.position]);
            }
        }
        if !target {
            continue;
        }
        if changed || flags != 0x0001 {
            return Err("unsupported duplicate/native/abstract/static callable method".to_owned());
        }
        let body = code.ok_or("callable method has no Code attribute")?;
        let mut code_reader = Reader {
            bytes: body,
            position: 0,
        };
        let max_stack = code_reader.u2()?;
        let max_locals = code_reader.u2()?;
        let code_size = code_reader.u4()? as usize;
        if max_stack != 1 || max_locals != 3 || code_reader.take(code_size)? != [0x03, 0xac] {
            return Err("callable method is no longer the expected host no-op".to_owned());
        }
        if code_reader.u2()? != 0 {
            return Err("unexpected callable exception table".to_owned());
        }
        code_reader.skip_attributes()?;
        if code_reader.position != body.len() {
            return Err("malformed callable Code attribute".to_owned());
        }
        output.extend_from_slice(&bytes[copied..method_start]);
        output.extend_from_slice(&(flags | 0x0100).to_be_bytes());
        output.extend_from_slice(&(name as u16).to_be_bytes());
        output.extend_from_slice(&(descriptor as u16).to_be_bytes());
        output.extend_from_slice(&(retained.len() as u16).to_be_bytes());
        for attribute in retained {
            output.extend_from_slice(attribute);
        }
        copied = reader.position;
        changed = true;
    }
    reader.skip_attributes()?;
    if reader.position != bytes.len() {
        return Err("trailing classfile data".to_owned());
    }
    if !changed {
        return Err("host callable method not found".to_owned());
    }
    output.extend_from_slice(&bytes[copied..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::native_callable;

    #[test]
    fn missing_header_and_truncated_constant_pool_are_rejected() {
        for length in 0..10 {
            assert!(native_callable(&vec![0; length]).is_err());
        }
        let truncated_utf8 = [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 69, 0, 2, 1, 0xff, 0xff];
        assert!(native_callable(&truncated_utf8).is_err());
    }

    #[test]
    fn unknown_constant_pool_tags_are_rejected() {
        let unsupported_constant = [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 69, 0, 2, 0xff];
        assert!(native_callable(&unsupported_constant).is_err());
    }
}

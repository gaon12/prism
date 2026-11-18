//! Accept only a single public X.509 Certificate DER envelope before embedding it.
//! This is a bounded structural check, not signature, validity, purpose or key validation.
//! Those semantic checks remain the responsibility of the certificate APIs.

pub const MAX_CERTIFICATE_DER_BYTES: usize = 16 * 1024;
type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy)]
struct Element<'a> {
    tag: u8,
    value: &'a [u8],
    encoded: &'a [u8],
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn read(&mut self) -> Result<Element<'a>> {
        let input = self.0;
        let tag = *input.first().ok_or("Truncated DER tag.")?;
        // X.509's envelope and supported primitive values use short-form tags.
        // Do not guess the structure of unsupported high-tag-number values.
        if tag == 0 || tag & 0x1f == 0x1f {
            return Err("Unsupported DER tag.");
        }
        let first = *input.get(1).ok_or("Truncated DER length.")?;
        let (length, header) = if first < 0x80 {
            (usize::from(first), 2)
        } else {
            let count = usize::from(first & 0x7f);
            if count == 0 || count > std::mem::size_of::<usize>() {
                return Err("Indefinite or excessive DER length.");
            }
            let bytes = input.get(2..2 + count).ok_or("Truncated DER length.")?;
            if bytes[0] == 0 {
                return Err("Noncanonical DER length.");
            }
            let mut length = 0usize;
            for byte in bytes {
                length = length
                    .checked_mul(256)
                    .and_then(|value| value.checked_add(usize::from(*byte)))
                    .ok_or("Excessive DER length.")?;
            }
            if length < 0x80 {
                return Err("Noncanonical DER length.");
            }
            (length, 2 + count)
        };
        let end = header.checked_add(length).ok_or("Excessive DER length.")?;
        let value = input.get(header..end).ok_or("Truncated DER value.")?;
        self.0 = &input[end..];
        Ok(Element {
            tag,
            value,
            encoded: &input[..end],
        })
    }

    fn expect(&mut self, tag: u8) -> Result<Element<'a>> {
        let value = self.read()?;
        if value.tag != tag {
            return Err("Not a public X.509 Certificate envelope.");
        }
        Ok(value)
    }

    fn finish(self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err("Unexpected data after the DER structure.")
        }
    }
}

fn integer(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty()
        || (bytes.len() > 1
            && ((bytes[0] == 0 && bytes[1] & 0x80 == 0)
                || (bytes[0] == 0xff && bytes[1] & 0x80 != 0)))
    {
        return Err("Noncanonical DER integer.");
    }
    Ok(())
}

fn bits(bytes: &[u8]) -> Result<()> {
    let unused = *bytes.first().ok_or("Missing DER bit-string padding.")?;
    if unused > 7
        || (bytes.len() == 1 && unused != 0)
        || (unused != 0 && bytes.last().unwrap() & ((1u8 << unused) - 1) != 0)
    {
        return Err("Invalid DER bit-string padding.");
    }
    Ok(())
}

fn oid(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.last().unwrap() & 0x80 != 0 {
        return Err("Invalid DER object identifier.");
    }
    let mut beginning = true;
    for byte in bytes {
        if beginning && *byte == 0x80 {
            return Err("Noncanonical DER object identifier.");
        }
        beginning = byte & 0x80 == 0;
    }
    Ok(())
}

fn canonical(element: Element<'_>, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err("Excessively nested DER.");
    }
    if element.tag & 0x20 != 0 {
        if element.tag & 0xc0 == 0 && !matches!(element.tag, 0x30 | 0x31) {
            return Err("Constructed encoding is not canonical DER.");
        }
        let mut children = Reader(element.value);
        let mut previous: Option<&[u8]> = None;
        while !children.0.is_empty() {
            let child = children.read()?;
            if element.tag == 0x31 && previous.is_some_and(|value| value > child.encoded) {
                return Err("Noncanonical DER set ordering.");
            }
            canonical(child, depth + 1)?;
            previous = Some(child.encoded);
        }
    } else {
        match element.tag {
            0x01 if element.value != [0] && element.value != [0xff] => {
                return Err("Noncanonical DER boolean.")
            }
            0x02 => integer(element.value)?,
            0x03 => bits(element.value)?,
            0x05 if !element.value.is_empty() => return Err("Nonempty DER NULL."),
            0x06 => oid(element.value)?,
            0x10 | 0x11 => return Err("Primitive encoding of a DER collection."),
            _ => {}
        }
    }
    Ok(())
}

fn algorithm(element: Element<'_>) -> Result<()> {
    let mut fields = Reader(element.value);
    fields.expect(0x06)?;
    if !fields.0.is_empty() {
        fields.read()?; // Optional algorithm-specific parameters, already DER-checked.
    }
    fields.finish()
}

fn name(element: Element<'_>) -> Result<()> {
    let mut rdns = Reader(element.value);
    while !rdns.0.is_empty() {
        let mut attributes = Reader(rdns.expect(0x31)?.value);
        if attributes.0.is_empty() {
            return Err("Empty distinguished-name attribute set.");
        }
        while !attributes.0.is_empty() {
            let mut attribute = Reader(attributes.expect(0x30)?.value);
            attribute.expect(0x06)?;
            attribute.read()?;
            attribute.finish()?;
        }
    }
    Ok(())
}

fn validity(element: Element<'_>) -> Result<()> {
    let mut fields = Reader(element.value);
    for _ in 0..2 {
        let time = fields.read()?;
        let length = match time.tag {
            0x17 => 13,
            0x18 => 15,
            _ => return Err("Invalid certificate validity envelope."),
        };
        if time.value.len() != length
            || time.value[length - 1] != b'Z'
            || !time.value[..length - 1].iter().all(u8::is_ascii_digit)
        {
            return Err("Noncanonical certificate time.");
        }
    }
    fields.finish()
}

fn extensions(element: Element<'_>) -> Result<()> {
    let mut explicit = Reader(element.value);
    let mut entries = Reader(explicit.expect(0x30)?.value);
    explicit.finish()?;
    if entries.0.is_empty() {
        return Err("Empty certificate extensions.");
    }
    while !entries.0.is_empty() {
        let mut fields = Reader(entries.expect(0x30)?.value);
        fields.expect(0x06)?;
        if fields.0.first() == Some(&0x01) && fields.expect(0x01)?.value != [0xff] {
            return Err("Default false extension flag must be omitted in DER.");
        }
        fields.expect(0x04)?;
        fields.finish()?;
    }
    Ok(())
}

fn tbs(element: Element<'_>) -> Result<()> {
    let mut fields = Reader(element.value);
    let version = if fields.0.first() == Some(&0xa0) {
        let mut explicit = Reader(fields.expect(0xa0)?.value);
        let version = explicit.expect(0x02)?;
        explicit.finish()?;
        match version.value {
            [version @ (1 | 2)] => *version,
            _ => return Err("Invalid or explicitly encoded default certificate version."),
        }
    } else {
        0
    };
    let serial = fields.expect(0x02)?;
    if serial.value[0] & 0x80 != 0 || serial.value.iter().all(|byte| *byte == 0) {
        return Err("Certificate serial number must be positive.");
    }
    algorithm(fields.expect(0x30)?)?;
    name(fields.expect(0x30)?)?;
    validity(fields.expect(0x30)?)?;
    name(fields.expect(0x30)?)?;
    let mut public_key = Reader(fields.expect(0x30)?.value);
    algorithm(public_key.expect(0x30)?)?;
    if public_key.expect(0x03)?.value.len() < 2 {
        return Err("Missing certificate public-key bits.");
    }
    public_key.finish()?;
    for tag in [0x81, 0x82] {
        if fields.0.first() == Some(&tag) {
            if version == 0 {
                return Err("Unique identifiers require a version 2 or 3 certificate.");
            }
            bits(fields.expect(tag)?.value)?;
        }
    }
    if fields.0.first() == Some(&0xa3) {
        if version != 2 {
            return Err("Extensions require a version 3 certificate.");
        }
        extensions(fields.expect(0xa3)?)?;
    }
    fields.finish()
}

/// Reject containers, private-key encodings, concatenated data and noncanonical lengths.
/// A successful result identifies only the public-certificate envelope and its field shape.
pub fn validate_certificate_der(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_CERTIFICATE_DER_BYTES {
        return Err("Expected one public DER .cer certificate of at most 16 KiB.");
    }
    let mut input = Reader(bytes);
    let outer = input.expect(0x30)?;
    input.finish()?;
    canonical(outer, 0)?;
    let mut certificate = Reader(outer.value);
    tbs(certificate.expect(0x30)?)?;
    algorithm(certificate.expect(0x30)?)?;
    let signature = certificate.expect(0x03)?;
    if signature.value.len() < 2 || signature.value[0] != 0 {
        return Err("Invalid certificate signature bit-string envelope.");
    }
    certificate.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
        let mut encoded = vec![tag];
        if value.len() < 128 {
            encoded.push(value.len() as u8);
        } else if value.len() <= 255 {
            encoded.extend([0x81, value.len() as u8]);
        } else {
            encoded.extend([0x82, (value.len() >> 8) as u8, value.len() as u8]);
        }
        encoded.extend(value);
        encoded
    }

    fn sequence(fields: &[Vec<u8>]) -> Vec<u8> {
        tlv(0x30, &fields.concat())
    }

    fn algorithm_id() -> Vec<u8> {
        sequence(&[tlv(0x06, &[0x2a, 3]), tlv(0x05, &[])])
    }

    // Clearly synthetic envelope: the key and signature bits have no cryptographic meaning.
    // This fixture contains no generated key, private-key material or store integration.
    fn certificate() -> Vec<u8> {
        let name = sequence(&[tlv(
            0x31,
            &sequence(&[
                tlv(0x06, &[0x55, 4, 3]),
                tlv(0x0c, b"Public envelope fixture"),
            ]),
        )]);
        let body = sequence(&[
            tlv(0xa0, &tlv(0x02, &[2])),
            tlv(0x02, &[1]),
            algorithm_id(),
            name.clone(),
            sequence(&[tlv(0x17, b"260101000000Z"), tlv(0x18, b"20360101000000Z")]),
            name,
            sequence(&[algorithm_id(), tlv(0x03, &[0, 1])]),
        ]);
        sequence(&[body, algorithm_id(), tlv(0x03, &[0, 1])])
    }

    #[test]
    fn accepts_only_the_synthetic_public_certificate_shape_without_claiming_semantics() {
        assert!(validate_certificate_der(&certificate()).is_ok());
    }

    #[test]
    fn rejects_private_key_containers_and_pkcs7_pkcs12_envelopes() {
        let pkcs8 = sequence(&[tlv(0x02, &[0]), algorithm_id(), tlv(0x04, &[1])]);
        let encrypted_pkcs8 = sequence(&[algorithm_id(), tlv(0x04, &[1])]);
        let mut rsa_fields = vec![tlv(0x02, &[1]); 9]; // Inert integers, not an RSA key.
        rsa_fields[0] = tlv(0x02, &[0]);
        let pkcs1 = sequence(&rsa_fields);
        let sec1 = sequence(&[tlv(0x02, &[1]), tlv(0x04, &[1])]);
        let cms = sequence(&[
            tlv(0x06, &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 7, 1]),
            tlv(0xa0, &tlv(0x04, &[])),
        ]);
        let pfx = sequence(&[tlv(0x02, &[3]), cms.clone(), sequence(&[])]);
        let disguised_key = sequence(&[pkcs1.clone(), algorithm_id(), tlv(0x03, &[0, 1])]);
        for bytes in [pkcs8, encrypted_pkcs8, pkcs1, sec1, cms, pfx, disguised_key] {
            assert!(validate_certificate_der(&bytes).is_err());
        }
        assert!(validate_certificate_der(b"-----BEGIN CERTIFICATE-----\nfixture\n").is_err());
    }

    #[test]
    fn rejects_trailing_container_data_even_after_a_valid_certificate() {
        let mut bytes = certificate();
        bytes.extend(sequence(&[
            tlv(0x02, &[0]),
            algorithm_id(),
            tlv(0x04, &[1]),
        ]));
        assert!(validate_certificate_der(&bytes).is_err());
        let mut outer = Reader(&certificate()).expect(0x30).unwrap().value.to_vec();
        outer.extend(tlv(0x04, b"unrelated trailing data"));
        assert!(validate_certificate_der(&tlv(0x30, &outer)).is_err());
    }

    #[test]
    fn rejects_noncanonical_lengths_truncation_oversize_and_excessive_nesting() {
        assert!(validate_certificate_der(&[]).is_err());
        assert!(validate_certificate_der(&vec![0x30; MAX_CERTIFICATE_DER_BYTES + 1]).is_err());
        let valid = certificate();
        for length in 0..valid.len() {
            assert!(validate_certificate_der(&valid[..length]).is_err());
        }
        for bytes in [
            vec![0x30, 0x80, 0, 0],
            vec![0x30, 0x81, 0],
            vec![0x30, 0x82, 0, 0x80],
            vec![0x30, 0x89, 1],
            vec![0x30, 0xff],
        ] {
            assert!(validate_certificate_der(&bytes).is_err());
        }
        let mut nested = tlv(0x05, &[]);
        for _ in 0..40 {
            nested = sequence(&[nested]);
        }
        assert!(validate_certificate_der(&nested).is_err());
    }
}

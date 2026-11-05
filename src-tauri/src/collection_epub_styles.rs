//! CSS tokens preserve author rules while resolving local resources.
use super::{check, css_uri, error, Book, ErrorCode, Result, MAX_DOCUMENT};
use cssparser::{Parser, ParserInput, ToCss, Token};
impl Book<'_> {
    pub(super) fn css(&mut self, base: &str, css: &str) -> Result<String> {
        let mut input = ParserInput::new(css);
        self.css_tokens(base, &mut Parser::new(&mut input), 0)
    }
    fn css_tokens(&mut self, base: &str, input: &mut Parser<'_, '_>, depth: u8) -> Result<String> {
        if depth > 32 {
            return Err(error(ErrorCode::Limit));
        }
        let mut output = String::new();
        while let Ok(token) = input.next_including_whitespace_and_comments().cloned() {
            check(self.cancel)?;
            match token {
                Token::UnquotedUrl(ref value) => {
                    let uri = self.resource(base, value, false)?;
                    output.push_str("url(");
                    Token::QuotedString(uri.into())
                        .to_css(&mut output)
                        .map_err(|_| error(ErrorCode::Unsupported))?;
                    output.push(')');
                }
                Token::Function(ref name) if name.eq_ignore_ascii_case("url") => {
                    let value = input
                        .parse_nested_block(|parser| {
                            parser
                                .expect_string_cloned()
                                .map_err(cssparser::ParseError::<()>::from)
                        })
                        .map_err(|_| error(ErrorCode::Unsupported))?;
                    let uri = self.resource(base, &value, false)?;
                    output.push_str("url(");
                    Token::QuotedString(uri.into())
                        .to_css(&mut output)
                        .map_err(|_| error(ErrorCode::Unsupported))?;
                    output.push(')');
                }
                Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock => {
                    let close = match token {
                        Token::SquareBracketBlock => ']',
                        Token::CurlyBracketBlock => '}',
                        _ => ')',
                    };
                    token
                        .to_css(&mut output)
                        .map_err(|_| error(ErrorCode::Unsupported))?;
                    let nested = input.parse_nested_block(|parser| self.css_tokens(base, parser, depth+1).map_err(|e| parser.new_custom_error::<_, crate::collection_store::LibraryError>(e))).map_err(|e| match e.kind { cssparser::ParseErrorKind::Custom(e) => e, _ => error(ErrorCode::Unsupported) })?;
                    output.push_str(&nested);
                    output.push(close);
                }
                Token::AtKeyword(ref name) if name.eq_ignore_ascii_case("import") => {
                    let reference = input
                        .expect_url_or_string()
                        .map_err(|_| error(ErrorCode::Unsupported))?
                        .to_string();
                    let condition = input.parse_until_before(cssparser::Delimiter::Semicolon, |parser| self.css_tokens(base, parser, depth+1).map_err(|e| parser.new_custom_error::<_, crate::collection_store::LibraryError>(e))).map_err(|_| error(ErrorCode::Unsupported))?;
                    let _ = input.try_parse(|parser| parser.expect_semicolon());
                    let uri = self.resource(base, &reference, true)?;
                    let mut css = css_uri(&uri)?;
                    if !condition.trim().is_empty() {
                        let mut condition_input = ParserInput::new(&condition);
                        let mut condition_parser = Parser::new(&mut condition_input);
                        let mut layer = None;
                        if condition_parser
                            .try_parse(|p| p.expect_ident_matching("layer"))
                            .is_ok()
                        {
                            layer = Some(String::new());
                        } else if condition_parser
                            .try_parse(|p| p.expect_function_matching("layer"))
                            .is_ok()
                        {
                            layer = Some(condition_parser.parse_nested_block(|p| self.css_tokens(base, p, depth+1).map_err(|e| p.new_custom_error::<_, crate::collection_store::LibraryError>(e))).map_err(|_| error(ErrorCode::Unsupported))?);
                        }
                        let supports = if condition_parser
                            .try_parse(|p| p.expect_function_matching("supports"))
                            .is_ok()
                        {
                            Some(condition_parser.parse_nested_block(|p| self.css_tokens(base, p, depth+1).map_err(|e| p.new_custom_error::<_, crate::collection_store::LibraryError>(e))).map_err(|_| error(ErrorCode::Unsupported))?)
                        } else {
                            None
                        };
                        let media = self.css_tokens(base, &mut condition_parser, depth + 1)?;
                        if !media.trim().is_empty() {
                            css = format!("@media {media}{{{css}}}");
                        }
                        if let Some(supports) = supports {
                            let condition = supports.trim();
                            let is_condition = condition.starts_with('(')
                                || condition.starts_with("not ")
                                || condition
                                    .find('(')
                                    .is_some_and(|at| !condition[..at].contains(':'));
                            css = if is_condition {
                                format!("@supports {condition}{{{css}}}")
                            } else {
                                format!("@supports ({condition}){{{css}}}")
                            };
                        }
                        if let Some(layer) = layer {
                            css = format!("@layer {layer}{{{css}}}");
                        }
                    }
                    output.push_str(&css);
                }
                _ if token.is_parse_error() => return Err(error(ErrorCode::Unsupported)),
                _ => {
                    token
                        .to_css(&mut output)
                        .map_err(|_| error(ErrorCode::Unsupported))?;
                }
            }
            if output.len() > MAX_DOCUMENT {
                return Err(error(ErrorCode::Limit));
            }
        }
        Ok(output)
    }
}

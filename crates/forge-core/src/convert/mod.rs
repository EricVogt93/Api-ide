//! Import/export: curl command parsing and generation, Postman collection /
//! environment import, plus code snippets (JS fetch, axios, Python requests,
//! HTTPie, Go, Java HttpClient).

mod common;
mod curl_in;
mod curl_out;
mod postman;
mod snippets;

pub use curl_in::{parse_curl, CurlParseError};
pub use curl_out::{to_curl, CurlExportOptions};
pub use postman::{
    parse_postman, parse_postman_environment, PostmanError, PostmanImport, PostmanItem,
};
pub use snippets::{generate, SnippetLang};

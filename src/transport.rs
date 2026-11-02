//! HTTP runtime boundary. Browsers enforce CORS and forbid redirects; dropping
//! a response/request aborts Fetch, including while its body is being streamed.
#[cfg(not(target_arch = "wasm32"))]
pub use reqwest::{Client, Error};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn client() -> Result<Client, Error> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn response_limit(
    request: reqwest::RequestBuilder,
    _limit: usize,
) -> reqwest::RequestBuilder {
    request
}
#[cfg(target_arch = "wasm32")]
pub use browser::{Client, Error};
#[cfg(target_arch = "wasm32")]
pub(crate) use browser::{client, response_limit};

#[cfg(target_arch = "wasm32")]
mod browser {
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        AbortController, ReadableStreamDefaultReader, RequestCredentials, RequestInit,
        RequestRedirect,
    };
    #[derive(Debug, thiserror::Error)]
    #[error("browser HTTP transport: {0}")]
    pub struct Error(String);
    impl From<JsValue> for Error {
        fn from(value: JsValue) -> Self {
            Self(value.as_string().unwrap_or_else(|| format!("{value:?}")))
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct Client;
    pub(crate) fn client() -> Result<Client, Error> {
        Ok(Client)
    }
    pub struct Request {
        url: String,
        method: &'static str,
        headers: Vec<(String, String)>,
        body: Option<String>,
        limit: usize,
    }
    impl Client {
        pub fn get(&self, url: impl ToString) -> Request {
            self.request("GET", url)
        }
        pub fn post(&self, url: impl ToString) -> Request {
            self.request("POST", url)
        }
        fn request(&self, method: &'static str, url: impl ToString) -> Request {
            Request {
                url: url.to_string(),
                method,
                headers: vec![],
                body: None,
                limit: usize::MAX,
            }
        }
    }
    pub(crate) fn response_limit(mut request: Request, limit: usize) -> Request {
        request.limit = limit;
        request
    }
    struct Abort(AbortController);
    impl Drop for Abort {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    impl Request {
        pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
            self.headers
                .push((name.as_ref().into(), value.as_ref().into()));
            self
        }
        pub fn body(mut self, body: String) -> Self {
            self.body = Some(body);
            self
        }
        pub async fn send(self) -> Result<Response, Error> {
            let abort = Abort(AbortController::new()?);
            let init = RequestInit::new();
            init.set_method(self.method);
            init.set_redirect(RequestRedirect::Error);
            init.set_credentials(RequestCredentials::Omit);
            init.set_signal(Some(&abort.0.signal()));
            if let Some(body) = self.body {
                init.set_body(&JsValue::from_str(&body));
            }
            let request = web_sys::Request::new_with_str_and_init(&self.url, &init)?;
            for (key, value) in self.headers {
                request.headers().set(&key, &value)?;
            }
            let global = js_sys::global();
            let fetch = js_sys::Reflect::get(&global, &JsValue::from_str("fetch"))?
                .dyn_into::<js_sys::Function>()?;
            let promise = fetch
                .call1(&global, &request)?
                .dyn_into::<js_sys::Promise>()?;
            let response = JsFuture::from(promise)
                .await?
                .dyn_into::<web_sys::Response>()?;
            if response.redirected() {
                return Err(Error("redirect forbidden".into()));
            }
            let status = reqwest::StatusCode::from_u16(response.status())
                .map_err(|e| Error(e.to_string()))?;
            let mut headers = reqwest::header::HeaderMap::new();
            if let Some(iterator) = js_sys::try_iter(&response.headers())? {
                for entry in iterator {
                    let pair = js_sys::Array::from(&entry?);
                    let key = pair
                        .get(0)
                        .as_string()
                        .ok_or_else(|| Error("invalid header".into()))?;
                    let value = pair
                        .get(1)
                        .as_string()
                        .ok_or_else(|| Error("invalid header".into()))?;
                    headers.insert(
                        reqwest::header::HeaderName::from_bytes(key.as_bytes())
                            .map_err(|e| Error(e.to_string()))?,
                        reqwest::header::HeaderValue::from_str(&value)
                            .map_err(|e| Error(e.to_string()))?,
                    );
                }
            }
            let reader = response
                .body()
                .map(|body| body.get_reader().dyn_into::<ReadableStreamDefaultReader>())
                .transpose()
                .map_err(|value| Error::from(JsValue::from(value)))?;
            Ok(Response {
                status,
                headers,
                reader,
                abort,
                remaining: self.limit,
            })
        }
    }
    pub struct Response {
        status: reqwest::StatusCode,
        headers: reqwest::header::HeaderMap,
        reader: Option<ReadableStreamDefaultReader>,
        abort: Abort,
        remaining: usize,
    }
    impl Response {
        pub fn status(&self) -> reqwest::StatusCode {
            self.status
        }
        pub fn headers(&self) -> &reqwest::header::HeaderMap {
            &self.headers
        }
        pub fn content_length(&self) -> Option<u64> {
            self.headers
                .get(reqwest::header::CONTENT_LENGTH)?
                .to_str()
                .ok()?
                .parse()
                .ok()
        }
        pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>, Error> {
            let Some(reader) = &self.reader else {
                return Ok(None);
            };
            let result = JsFuture::from(reader.read()).await?;
            if js_sys::Reflect::get(&result, &JsValue::from_str("done"))?.as_bool() == Some(true) {
                self.reader = None;
                return Ok(None);
            }
            let value = js_sys::Reflect::get(&result, &JsValue::from_str("value"))?
                .dyn_into::<js_sys::Uint8Array>()?;
            let length = value.length() as usize;
            // Copy at most the budget plus one sentinel byte. Existing callers
            // return their normal size-limit error, without allocating a giant
            // browser-provided chunk in Wasm memory.
            if length > self.remaining {
                self.abort.0.abort();
                return Ok(Some(
                    value
                        .subarray(
                            0,
                            self.remaining.saturating_add(1).min(u32::MAX as usize) as u32,
                        )
                        .to_vec(),
                ));
            }
            self.remaining -= length;
            Ok(Some(value.to_vec()))
        }
    }
}

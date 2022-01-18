use std::{
    future::{ready, Ready},
    marker::PhantomData,
    rc::Rc,
};

use actix_service::{
    boxed::{self, BoxFuture, RcService},
    forward_ready, Service, Transform,
};
use actix_web::{
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    Error,
};
use futures_core::Future;

/// Middleware transform for [`from_fn`].
pub struct MiddlewareFn<F> {
    mw_fn: Rc<F>,
}

impl<S, F, Fut, B, B2> Transform<S, ServiceRequest> for MiddlewareFn<F>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    F: Fn(ServiceRequest, Next<B>) -> Fut + 'static,
    Fut: Future<Output = Result<ServiceResponse<B2>, Error>>,
    B2: MessageBody,
{
    type Response = ServiceResponse<B2>;
    type Error = Error;
    type Transform = MiddlewareFnService<F, B>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(MiddlewareFnService {
            service: boxed::rc_service(service),
            mw_fn: Rc::clone(&self.mw_fn),
            _phantom: PhantomData,
        }))
    }
}

/// Middleware service for [`from_fn`].
pub struct MiddlewareFnService<F, B> {
    service: RcService<ServiceRequest, ServiceResponse<B>, Error>,
    mw_fn: Rc<F>,
    _phantom: PhantomData<B>,
}

impl<F, Fut, B, B2> Service<ServiceRequest> for MiddlewareFnService<F, B>
where
    F: Fn(ServiceRequest, Next<B>) -> Fut + 'static,
    Fut: Future<Output = Result<ServiceResponse<B2>, Error>>,
    B2: MessageBody,
{
    type Response = ServiceResponse<B2>;
    type Error = Error;
    type Future = Fut;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        (self.mw_fn)(
            req,
            Next::<B> {
                service: Rc::clone(&self.service),
            },
        )
    }
}

/// Wraps the "next" service in the middleware chain.
pub struct Next<B> {
    service: RcService<ServiceRequest, ServiceResponse<B>, Error>,
}

impl<B> Service<ServiceRequest> for Next<B> {
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = BoxFuture<Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        self.service.call(req)
    }
}

/// Wraps an async function to be used as a middleware.
///
/// The wrapped function should have the following form:
/// ```ignore
/// use actix_web_lab::middleware::Next;
///
/// async fn my_mw(req: ServiceRequest, next: Next<B>) -> Result<ServiceResponse<B>, Error> {
///     // pre-processing
///     next.call(req).await
///     // post-processing
/// }
/// ```
///
/// Then use in an app builder like this:
/// ```
/// use actix_web::{
///     App, Error,
///     dev::{ServiceRequest, ServiceResponse, Service as _},
/// };
/// use actix_web_lab::middleware::from_fn;
/// # use actix_web_lab::middleware::Next;
/// # async fn my_mw<B>(req: ServiceRequest, next: Next<B>) -> Result<ServiceResponse<B>, Error> {
/// #     next.call(req).await
/// # }
///
/// App::new()
///     .wrap(from_fn(my_mw))
/// # ;
/// ```
pub fn from_fn<F>(mw_fn: F) -> MiddlewareFn<F> {
    MiddlewareFn {
        mw_fn: Rc::new(mw_fn),
    }
}

#[cfg(test)]
mod tests {
    use actix_web::{
        http::header::{self, HeaderValue},
        middleware::Logger,
        test, web, App, HttpResponse,
    };

    use super::*;

    #[actix_web::test]
    async fn feels_good() {
        async fn noop<B>(req: ServiceRequest, next: Next<B>) -> Result<ServiceResponse<B>, Error> {
            next.call(req).await
        }

        async fn add_res_header<B>(
            req: ServiceRequest,
            next: Next<B>,
        ) -> Result<ServiceResponse<B>, Error> {
            let mut res = next.call(req).await?;
            res.headers_mut()
                .insert(header::WARNING, HeaderValue::from_static("42"));
            Ok(res)
        }

        async fn mutate_body_type(
            req: ServiceRequest,
            next: Next<impl MessageBody + 'static>,
        ) -> Result<ServiceResponse<impl MessageBody>, Error> {
            let res = next.call(req).await?;
            Ok(res.map_into_left_body::<()>())
        }

        let app = test::init_service(
            App::new()
                .wrap(from_fn(mutate_body_type))
                .wrap(from_fn(add_res_header))
                .wrap(Logger::default())
                .wrap(from_fn(noop))
                .default_service(web::to(HttpResponse::NotFound)),
        )
        .await;

        let req = test::TestRequest::default().to_request();
        let res = test::call_service(&app, req).await;
        assert!(res.headers().contains_key(header::WARNING));
    }
}

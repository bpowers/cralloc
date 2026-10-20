//! The order service on actix-web: one single-threaded runtime per worker thread, so blocks
//! cross threads only through the shared store.
use actix_web::{App, HttpServer, web};
use screen_servers::{NewOrder, Store, port};

async fn create(store: web::Data<Store>, new: web::Json<NewOrder>) -> String {
    store.create(new.into_inner())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let store = web::Data::new(Store::default());
    HttpServer::new(move || App::new().app_data(store.clone()).route("/orders", web::post().to(create)))
        .bind(("127.0.0.1", port()))?
        .run()
        .await
}

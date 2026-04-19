mod db;

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use askama::Template;
use axum::{
    body::Body,
    extract::{Host, Path, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect},
    routing::{delete, get, post},
    Form, Router,
};
use serde::Deserialize;
use sqlx::sqlite::SqlitePool;
use std::sync::Arc;
use time::Duration;
use tower_sessions::{Expiry, MemoryStore, Session, SessionManagerLayer};

// Session key for authentication status
const SESSION_AUTH_KEY: &str = "authenticated";

// Application state
pub struct AppState {
    pub pool: SqlitePool,
    pub admin_password_hash: String,
}

/// Hash a password using Argon2id
#[allow(dead_code)]
fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .expect("Failed to hash password")
        .to_string()
}

/// Verify a password against a hash
fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed_hash) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok(),
        Err(_) => false,
    }
}

// Templates
#[derive(Template)]
#[template(path = "admin_list.html")]
struct AdminListTemplate {
    prompts: Vec<db::Prompt>,
}

#[derive(Template)]
#[template(path = "admin_new.html")]
struct AdminNewTemplate;

#[derive(Template)]
#[template(path = "admin_detail.html")]
struct AdminDetailTemplate {
    prompt: db::Prompt,
    feedback_list: Vec<db::Feedback>,
    feedback_url: String,
}

#[derive(Template)]
#[template(path = "feedback_form.html")]
struct FeedbackFormTemplate {
    prompt: db::Prompt,
}

#[derive(Template)]
#[template(path = "feedback_success_partial.html")]
struct FeedbackSuccessPartialTemplate;

#[derive(Template)]
#[template(path = "feedback_list_partial.html")]
struct FeedbackListPartialTemplate {
    feedback_list: Vec<db::Feedback>,
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: Option<String>,
}

// Form data
#[derive(Deserialize)]
struct NewPromptForm {
    title: String,
    description: String,
}

#[derive(Deserialize)]
struct FeedbackForm {
    content: String,
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
}

// Authentication middleware
async fn require_auth(
    session: Session,
    request: Request<Body>,
    next: Next,
) -> Result<impl IntoResponse, Redirect> {
    let authenticated: bool = session
        .get(SESSION_AUTH_KEY)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);

    if authenticated {
        Ok(next.run(request).await)
    } else {
        Err(Redirect::to("/login"))
    }
}

// Login handlers
async fn login_form() -> impl IntoResponse {
    let template = LoginTemplate { error: None };
    Html(template.render().unwrap())
}

async fn login_submit(
    State(state): State<Arc<AppState>>,
    session: Session,
    Form(form): Form<LoginForm>,
) -> impl IntoResponse {
    if verify_password(&form.password, &state.admin_password_hash) {
        session.insert(SESSION_AUTH_KEY, true).await.unwrap();
        Redirect::to("/admin").into_response()
    } else {
        let template = LoginTemplate {
            error: Some("Invalid password".to_string()),
        };
        Html(template.render().unwrap()).into_response()
    }
}

async fn logout(session: Session) -> impl IntoResponse {
    session.delete().await.unwrap();
    Redirect::to("/login")
}

// Handlers
async fn admin_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match db::get_all_prompts(&state.pool).await {
        Ok(prompts) => {
            let template = AdminListTemplate { prompts };
            Html(template.render().unwrap())
        }
        Err(_) => Html("Error loading prompts".to_string()),
    }
}

async fn admin_new_form() -> impl IntoResponse {
    let template = AdminNewTemplate;
    Html(template.render().unwrap())
}

async fn admin_new_submit(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NewPromptForm>,
) -> impl IntoResponse {
    match db::create_prompt(&state.pool, &form.title, &form.description).await {
        Ok(prompt) => Redirect::to(&format!("/admin/prompt/{}", prompt.id)),
        Err(_) => Redirect::to("/admin"),
    }
}

async fn admin_detail(
    State(state): State<Arc<AppState>>,
    Host(host): Host,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let prompt = match db::get_prompt_by_id(&state.pool, &id).await {
        Ok(Some(p)) => p,
        _ => return Html("Prompt not found".to_string()),
    };

    let feedback_list = db::get_feedback_for_prompt(&state.pool, &id)
        .await
        .unwrap_or_default();

    let protocol = if host.contains("localhost") || host.contains("127.0.0.1") {
        "http"
    } else {
        "https"
    };
    let feedback_url = format!("{}://{}/feedback/{}", protocol, host, id);

    let template = AdminDetailTemplate {
        prompt,
        feedback_list,
        feedback_url,
    };
    Html(template.render().unwrap())
}

async fn feedback_form(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match db::get_prompt_by_id(&state.pool, &id).await {
        Ok(Some(prompt)) => {
            let template = FeedbackFormTemplate { prompt };
            Html(template.render().unwrap())
        }
        _ => Html("Prompt not found".to_string()),
    }
}

async fn feedback_submit(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Form(form): Form<FeedbackForm>,
) -> impl IntoResponse {
    // Verify prompt exists
    match db::get_prompt_by_id(&state.pool, &id).await {
        Ok(Some(_)) => {}
        _ => return Html("Prompt not found".to_string()),
    }

    match db::create_feedback(&state.pool, &id, &form.content).await {
        Ok(_) => {
            let template = FeedbackSuccessPartialTemplate;
            Html(template.render().unwrap())
        }
        Err(_) => Html("Error submitting feedback".to_string()),
    }
}

async fn api_delete_prompt(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match db::delete_prompt(&state.pool, &id).await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn api_get_feedback(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let feedback_list = db::get_feedback_for_prompt(&state.pool, &id)
        .await
        .unwrap_or_default();

    let template = FeedbackListPartialTemplate { feedback_list };
    Html(template.render().unwrap())
}

async fn index() -> impl IntoResponse {
    Redirect::to("/admin")
}

/// Create the application router with the given state
pub fn create_router(state: Arc<AppState>) -> Router {
    // Session store
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(false) // Set to true in production with HTTPS
        .with_expiry(Expiry::OnInactivity(Duration::hours(24)));

    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/login", get(login_form).post(login_submit))
        .route("/feedback/:id", get(feedback_form).post(feedback_submit));

    // Protected routes (auth required)
    let protected_routes = Router::new()
        .route("/", get(index))
        .route("/admin", get(admin_list))
        .route("/admin/new", get(admin_new_form).post(admin_new_submit))
        .route("/admin/prompt/:id", get(admin_detail))
        .route("/api/prompts/:id", delete(api_delete_prompt))
        .route("/api/feedback/:id", get(api_get_feedback))
        .route("/logout", post(logout))
        .route_layer(middleware::from_fn(require_auth));

    // Combine routes, add state, apply session layer
    public_routes
        .merge(protected_routes)
        .with_state(state)
        .layer(session_layer)
}

#[tokio::main]
async fn main() {
    // Load admin password hash from environment
    let admin_password_hash = std::env::var("ADMIN_PASSWORD_HASH")
        .expect("ADMIN_PASSWORD_HASH environment variable must be set");

    // Initialize database
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:feedback.db?mode=rwc".to_string());

    let pool = db::init_db(&database_url)
        .await
        .expect("Failed to initialize database");

    let state = Arc::new(AppState {
        pool,
        admin_password_hash,
    });

    // Build router
    let app = create_router(state);

    let addr = "0.0.0.0:3000";
    println!("Server running at http://localhost:3000");
    println!("Admin interface: http://localhost:3000/admin");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TEST_PASSWORD: &str = "test_password";

    async fn setup_test_app() -> (Router, Arc<AppState>) {
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        let admin_password_hash = hash_password(TEST_PASSWORD);
        let state = Arc::new(AppState {
            pool,
            admin_password_hash,
        });
        let app = create_router(state.clone());
        (app, state)
    }

    // Authentication tests

    #[tokio::test]
    async fn test_login_page_accessible() {
        let (app, _) = setup_test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("Admin Login"));
        assert!(body_str.contains("Password"));
    }

    #[tokio::test]
    async fn test_login_with_wrong_password() {
        let (app, _) = setup_test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("password=wrong_password"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("Invalid password"));
    }

    #[tokio::test]
    async fn test_protected_routes_redirect_to_login() {
        let (app, _) = setup_test_app().await;

        // Test / redirects to login
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get("location").unwrap(), "/login");
    }

    #[tokio::test]
    async fn test_admin_list_requires_auth() {
        let (app, _) = setup_test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get("location").unwrap(), "/login");
    }

    #[tokio::test]
    async fn test_admin_new_requires_auth() {
        let (app, _) = setup_test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/new")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get("location").unwrap(), "/login");
    }

    #[tokio::test]
    async fn test_admin_detail_requires_auth() {
        let (app, state) = setup_test_app().await;

        let prompt = db::create_prompt(&state.pool, "Test", "Description")
            .await
            .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .uri(&format!("/admin/prompt/{}", prompt.id))
                    .header("host", "localhost:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get("location").unwrap(), "/login");
    }

    // Public routes tests (feedback routes should work without auth)

    #[tokio::test]
    async fn test_feedback_form_public() {
        let (app, state) = setup_test_app().await;

        let prompt = db::create_prompt(&state.pool, "Feedback Test", "Give us feedback")
            .await
            .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .uri(&format!("/feedback/{}", prompt.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("Feedback Test"));
        assert!(body_str.contains("Give us feedback"));
        assert!(body_str.contains("Your Feedback"));
    }

    #[tokio::test]
    async fn test_feedback_form_not_found() {
        let (app, _) = setup_test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/feedback/nonexistent-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("Prompt not found"));
    }

    #[tokio::test]
    async fn test_feedback_submit_public() {
        let (app, state) = setup_test_app().await;

        let prompt = db::create_prompt(&state.pool, "Submit Test", "Description")
            .await
            .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/feedback/{}", prompt.id))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("content=This+is+my+feedback"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("Thank you!"));
        assert!(body_str.contains("Your feedback has been submitted successfully"));

        // Verify feedback was created
        let feedback_list = db::get_feedback_for_prompt(&state.pool, &prompt.id)
            .await
            .unwrap();
        assert_eq!(feedback_list.len(), 1);
        assert_eq!(feedback_list[0].content, "This is my feedback");
    }

    #[tokio::test]
    async fn test_feedback_submit_prompt_not_found() {
        let (app, _) = setup_test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/feedback/nonexistent-id")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("content=This+is+my+feedback"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("Prompt not found"));
    }

    // Password utility tests

    #[tokio::test]
    async fn test_password_hash_and_verify() {
        let password = "my_secret_password";
        let hash = hash_password(password);

        assert!(verify_password(password, &hash));
        assert!(!verify_password("wrong_password", &hash));
    }
}

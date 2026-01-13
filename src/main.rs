mod db;

use askama::Template;
use axum::{
    extract::{Host, Path, State},
    response::{Html, IntoResponse, Redirect},
    routing::get,
    Form, Router,
};
use serde::Deserialize;
use sqlx::sqlite::SqlitePool;
use std::sync::Arc;

// Application state
pub struct AppState {
    pub pool: SqlitePool,
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
#[template(path = "feedback_success.html")]
struct FeedbackSuccessTemplate;

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
            let template = FeedbackSuccessTemplate;
            Html(template.render().unwrap())
        }
        Err(_) => Html("Error submitting feedback".to_string()),
    }
}

async fn index() -> impl IntoResponse {
    Redirect::to("/admin")
}

/// Create the application router with the given state
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/admin", get(admin_list))
        .route("/admin/new", get(admin_new_form).post(admin_new_submit))
        .route("/admin/prompt/:id", get(admin_detail))
        .route("/feedback/:id", get(feedback_form).post(feedback_submit))
        .with_state(state)
}

#[tokio::main]
async fn main() {
    // Initialize database
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:feedback.db?mode=rwc".to_string());

    let pool = db::init_db(&database_url)
        .await
        .expect("Failed to initialize database");

    let state = Arc::new(AppState { pool });

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

    async fn setup_test_app() -> (Router, Arc<AppState>) {
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        let state = Arc::new(AppState { pool });
        let app = create_router(state.clone());
        (app, state)
    }

    #[tokio::test]
    async fn test_index_redirects_to_admin() {
        let (app, _) = setup_test_app().await;

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers().get("location").unwrap(), "/admin");
    }

    #[tokio::test]
    async fn test_admin_list_empty() {
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

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("Feedback Prompts"));
        assert!(body_str.contains("No prompts yet"));
    }

    #[tokio::test]
    async fn test_admin_list_with_prompts() {
        let (app, state) = setup_test_app().await;

        // Create a prompt directly in the database
        db::create_prompt(&state.pool, "Test Prompt", "Test Description")
            .await
            .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("Test Prompt"));
        assert!(body_str.contains("Test Description"));
    }

    #[tokio::test]
    async fn test_admin_new_form() {
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

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("Create New Prompt"));
        assert!(body_str.contains("Title"));
        assert!(body_str.contains("Description"));
    }

    #[tokio::test]
    async fn test_admin_new_submit() {
        let (app, state) = setup_test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/new")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("title=New+Prompt&description=New+Description"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        // Verify prompt was created
        let prompts = db::get_all_prompts(&state.pool).await.unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].title, "New Prompt");
        assert_eq!(prompts[0].description, "New Description");
    }

    #[tokio::test]
    async fn test_admin_detail() {
        let (app, state) = setup_test_app().await;

        let prompt = db::create_prompt(&state.pool, "Detail Test", "Detail Description")
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

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("Detail Test"));
        assert!(body_str.contains("Detail Description"));
        assert!(body_str.contains(&format!("/feedback/{}", prompt.id)));
    }

    #[tokio::test]
    async fn test_admin_detail_not_found() {
        let (app, _) = setup_test_app().await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/prompt/nonexistent-id")
                    .header("host", "localhost:3000")
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
    async fn test_feedback_form() {
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
    async fn test_feedback_submit() {
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

        assert!(body_str.contains("Thank You"));

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

    #[tokio::test]
    async fn test_admin_detail_shows_feedback() {
        let (app, state) = setup_test_app().await;

        let prompt = db::create_prompt(&state.pool, "With Feedback", "Description")
            .await
            .unwrap();

        db::create_feedback(&state.pool, &prompt.id, "First response")
            .await
            .unwrap();
        db::create_feedback(&state.pool, &prompt.id, "Second response")
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

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = String::from_utf8(body.to_vec()).unwrap();

        assert!(body_str.contains("First response"));
        assert!(body_str.contains("Second response"));
        assert!(body_str.contains("Feedback Responses (2)"));
    }
}

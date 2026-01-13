use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePool, FromRow};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Prompt {
    pub id: String,
    pub title: String,
    pub description: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Feedback {
    pub id: String,
    pub prompt_id: String,
    pub content: String,
    pub created_at: String,
}

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePool::connect(database_url).await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS prompts (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            description TEXT NOT NULL,
            created_at TEXT NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS feedback (
            id TEXT PRIMARY KEY,
            prompt_id TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (prompt_id) REFERENCES prompts(id)
        )
        "#,
    )
    .execute(&pool)
    .await?;

    Ok(pool)
}

pub async fn create_prompt(
    pool: &SqlitePool,
    title: &str,
    description: &str,
) -> Result<Prompt, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();

    sqlx::query("INSERT INTO prompts (id, title, description, created_at) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(title)
        .bind(description)
        .bind(&created_at)
        .execute(pool)
        .await?;

    Ok(Prompt {
        id,
        title: title.to_string(),
        description: description.to_string(),
        created_at,
    })
}

pub async fn get_all_prompts(pool: &SqlitePool) -> Result<Vec<Prompt>, sqlx::Error> {
    sqlx::query_as::<_, Prompt>(
        "SELECT id, title, description, created_at FROM prompts ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await
}

pub async fn get_prompt_by_id(pool: &SqlitePool, id: &str) -> Result<Option<Prompt>, sqlx::Error> {
    sqlx::query_as::<_, Prompt>(
        "SELECT id, title, description, created_at FROM prompts WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn create_feedback(
    pool: &SqlitePool,
    prompt_id: &str,
    content: &str,
) -> Result<Feedback, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();

    sqlx::query("INSERT INTO feedback (id, prompt_id, content, created_at) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(prompt_id)
        .bind(content)
        .bind(&created_at)
        .execute(pool)
        .await?;

    Ok(Feedback {
        id,
        prompt_id: prompt_id.to_string(),
        content: content.to_string(),
        created_at,
    })
}

pub async fn get_feedback_for_prompt(
    pool: &SqlitePool,
    prompt_id: &str,
) -> Result<Vec<Feedback>, sqlx::Error> {
    sqlx::query_as::<_, Feedback>(
        "SELECT id, prompt_id, content, created_at FROM feedback WHERE prompt_id = ? ORDER BY created_at DESC",
    )
    .bind(prompt_id)
    .fetch_all(pool)
    .await
}

pub async fn delete_prompt(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    // Delete all feedback for this prompt first (foreign key constraint)
    sqlx::query("DELETE FROM feedback WHERE prompt_id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    // Delete the prompt
    sqlx::query("DELETE FROM prompts WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_test_db() -> SqlitePool {
        init_db("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn test_create_prompt() {
        let pool = setup_test_db().await;

        let prompt = create_prompt(&pool, "Test Title", "Test Description")
            .await
            .unwrap();

        assert_eq!(prompt.title, "Test Title");
        assert_eq!(prompt.description, "Test Description");
        assert!(!prompt.id.is_empty());
        assert!(!prompt.created_at.is_empty());
    }

    #[tokio::test]
    async fn test_get_all_prompts_empty() {
        let pool = setup_test_db().await;

        let prompts = get_all_prompts(&pool).await.unwrap();

        assert!(prompts.is_empty());
    }

    #[tokio::test]
    async fn test_get_all_prompts() {
        let pool = setup_test_db().await;

        create_prompt(&pool, "First", "First desc").await.unwrap();
        create_prompt(&pool, "Second", "Second desc").await.unwrap();

        let prompts = get_all_prompts(&pool).await.unwrap();

        assert_eq!(prompts.len(), 2);
        // Should be ordered by created_at DESC (most recent first)
        assert_eq!(prompts[0].title, "Second");
        assert_eq!(prompts[1].title, "First");
    }

    #[tokio::test]
    async fn test_get_prompt_by_id() {
        let pool = setup_test_db().await;

        let created = create_prompt(&pool, "Test", "Description").await.unwrap();

        let found = get_prompt_by_id(&pool, &created.id).await.unwrap();

        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.id, created.id);
        assert_eq!(found.title, "Test");
        assert_eq!(found.description, "Description");
    }

    #[tokio::test]
    async fn test_get_prompt_by_id_not_found() {
        let pool = setup_test_db().await;

        let found = get_prompt_by_id(&pool, "nonexistent-id").await.unwrap();

        assert!(found.is_none());
    }

    #[tokio::test]
    async fn test_create_feedback() {
        let pool = setup_test_db().await;

        let prompt = create_prompt(&pool, "Test", "Description").await.unwrap();

        let feedback = create_feedback(&pool, &prompt.id, "Great feedback!")
            .await
            .unwrap();

        assert_eq!(feedback.prompt_id, prompt.id);
        assert_eq!(feedback.content, "Great feedback!");
        assert!(!feedback.id.is_empty());
        assert!(!feedback.created_at.is_empty());
    }

    #[tokio::test]
    async fn test_get_feedback_for_prompt_empty() {
        let pool = setup_test_db().await;

        let prompt = create_prompt(&pool, "Test", "Description").await.unwrap();

        let feedback_list = get_feedback_for_prompt(&pool, &prompt.id).await.unwrap();

        assert!(feedback_list.is_empty());
    }

    #[tokio::test]
    async fn test_get_feedback_for_prompt() {
        let pool = setup_test_db().await;

        let prompt = create_prompt(&pool, "Test", "Description").await.unwrap();

        create_feedback(&pool, &prompt.id, "First feedback")
            .await
            .unwrap();
        create_feedback(&pool, &prompt.id, "Second feedback")
            .await
            .unwrap();

        let feedback_list = get_feedback_for_prompt(&pool, &prompt.id).await.unwrap();

        assert_eq!(feedback_list.len(), 2);
        // Should be ordered by created_at DESC
        assert_eq!(feedback_list[0].content, "Second feedback");
        assert_eq!(feedback_list[1].content, "First feedback");
    }

    #[tokio::test]
    async fn test_feedback_isolation_between_prompts() {
        let pool = setup_test_db().await;

        let prompt1 = create_prompt(&pool, "Prompt 1", "Desc 1").await.unwrap();
        let prompt2 = create_prompt(&pool, "Prompt 2", "Desc 2").await.unwrap();

        create_feedback(&pool, &prompt1.id, "Feedback for prompt 1")
            .await
            .unwrap();
        create_feedback(&pool, &prompt2.id, "Feedback for prompt 2")
            .await
            .unwrap();

        let feedback1 = get_feedback_for_prompt(&pool, &prompt1.id).await.unwrap();
        let feedback2 = get_feedback_for_prompt(&pool, &prompt2.id).await.unwrap();

        assert_eq!(feedback1.len(), 1);
        assert_eq!(feedback1[0].content, "Feedback for prompt 1");

        assert_eq!(feedback2.len(), 1);
        assert_eq!(feedback2[0].content, "Feedback for prompt 2");
    }
}

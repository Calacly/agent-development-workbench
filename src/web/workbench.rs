use askama::Template;
use axum::{extract::State, response::Html, routing::get, Extension, Json, Router};

use super::{render, Layout, WebResult};
use crate::{error::AppError, middleware::auth::AuthUser, workbench::WorkbenchData, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workbench", get(page))
        .route("/workbench/data", get(data))
}

#[derive(Template)]
#[template(path = "workbench.html")]
struct WorkbenchPage {
    layout: Layout,
}

async fn page(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> WebResult<Html<String>> {
    render(WorkbenchPage {
        layout: Layout::for_user(&state, &user, "/workbench").await?,
    })
}

async fn data(Extension(_user): Extension<AuthUser>) -> Result<Json<WorkbenchData>, AppError> {
    crate::workbench::workbench_data()
        .map(Json)
        .map_err(|error| AppError::Internal(format!("Invalid workbench fixture: {error}")))
}

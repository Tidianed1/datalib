//! `POST /api/probe` and `GET /api/probe/{id}`: a provider's probe
//! (`datalib-step probe`) as a job the wizard polls, so a list that
//! pages for a minute can say how far it has got. "Check connection"
//! asks for the account alone; a picker's "Load" asks for one list.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use datalib_probe::{ProbeList, ProbeProgress};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use crate::connect::{err, error_chain, scrub, tail, validated_type};
use crate::AppState;

/// A probe that has not answered by now is not going to.
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize)]
pub struct ProbeRequest {
    /// The group's `type`: the provider word (`slack`, `email`, …).
    #[serde(rename = "type")]
    pub source_type: String,
    /// The provider's **download** params, exactly as they would be
    /// written under `[steps.params]`. Download-shaped even when the
    /// wizard is filling in a render step: a render step's own params
    /// hold no credentials, and the labels its filter can name are the
    /// ones the account has.
    #[serde(default)]
    pub params: Value,
    /// The list to load as well; absent for "Check connection".
    #[serde(default)]
    pub list: Option<ProbeList>,
}

/// How one probe is going. The UI switches on these words, and the
/// poll endpoint reaps a probe as soon as it is no longer `Running`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    Running,
    Ok,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeStatus {
    pub id: String,
    pub status: ProbeState,
    /// How far a list has got, once it has said.
    pub progress: Option<ProbeProgress>,
    /// The `ProbeReport`, once `Ok`.
    pub report: Option<Value>,
    /// What the step printed on failure: its error chain.
    pub error: Option<String>,
}

type Slot = Arc<Mutex<ProbeStatus>>;

/// The probes in flight, by id. A global for the reason `connect.rs`
/// keeps its login attempts in one: a job outlives the request that
/// started it, and `AppState` is cloned per request.
fn jobs() -> &'static Mutex<HashMap<String, Slot>> {
    static JOBS: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn start_probe(
    State(s): State<AppState>,
    Json(req): Json<ProbeRequest>,
) -> Result<Json<ProbeStatus>, (StatusCode, Json<Value>)> {
    let source_type = validated_type(&req.source_type)?;
    let step_bin = crate::binaries::resolve_step_bin().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no `datalib-step` binary found (set $DATALIB_STEP_BIN or $DATALIB_BINARY_DIR). \
             Checking a connection runs the provider's own probe, so it needs the step binary \
             the pipeline uses.",
        )
    })?;
    let params = serde_json::to_string(&req.params).unwrap_or_else(|_| "{}".to_string());
    // The wizard's typed credentials are in here; an owner-only file
    // keeps them off argv, where `ps` would show them to every user.
    let params_file = datalib_dag::subprocess::write_params_file(
        &s.root,
        &format!("probe_{source_type}"),
        &params,
    )
    .map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("params file: {e:#}"),
        )
    })?;

    let mut cmd = Command::new(step_bin);
    cmd.arg("probe").arg(&source_type);
    if let Some(list) = req.list {
        cmd.arg("--list").arg(list.as_str());
    }
    cmd.arg(datalib_dag::subprocess::PARAMS_FILE_FLAG)
        .arg(params_file.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = cmd
        .spawn()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")))?;

    let id = uuid::Uuid::new_v4().to_string();
    let status = ProbeStatus {
        id: id.clone(),
        status: ProbeState::Running,
        progress: None,
        report: None,
        error: None,
    };
    let slot = Arc::new(Mutex::new(status.clone()));
    jobs()
        .lock()
        .expect("probe jobs mutex")
        .insert(id, slot.clone());
    tokio::spawn(async move {
        // Held until the step exits: dropping it deletes the file.
        let _params_file = params_file;
        run(child, &slot, &source_type).await;
    });
    Ok(Json(status))
}

/// Wait for the step, keeping the slot's progress current from its
/// stderr, and record how it ended.
async fn run(mut child: tokio::process::Child, slot: &Slot, source_type: &str) {
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let read_stdout = async {
        let mut out = String::new();
        let _ = BufReader::new(stdout).read_to_string(&mut out).await;
        out
    };
    let read_stderr = async {
        let mut rest = String::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match ProbeProgress::parse_line(&line) {
                Some(p) => slot.lock().expect("probe slot mutex").progress = Some(p),
                None => {
                    rest.push_str(&line);
                    rest.push('\n');
                }
            }
        }
        rest
    };
    let finished = tokio::time::timeout(PROBE_TIMEOUT, async {
        tokio::join!(read_stdout, read_stderr, child.wait())
    })
    .await;
    let (stdout, stderr, exit) = match finished {
        Ok(done) => done,
        Err(_) => {
            let _ = child.kill().await;
            return fail(
                slot,
                "the probe did not answer within two minutes".to_string(),
            );
        }
    };
    let succeeded = matches!(exit, Ok(status) if status.success());
    if !succeeded {
        // The step prints its error chain to stderr; that chain is the
        // useful message ("Gmail users.getProfile: HTTP 401 …"), so
        // pass it through rather than replacing it with our own.
        tracing::error!(
            source_type = %source_type,
            "probe failed: {}",
            scrub(&error_chain(&stderr))
        );
        return fail(slot, tail(&stderr));
    }
    match serde_json::from_str::<Value>(&stdout) {
        Ok(report) => {
            let mut slot = slot.lock().expect("probe slot mutex");
            slot.status = ProbeState::Ok;
            slot.report = Some(report);
        }
        Err(e) => fail(
            slot,
            format!("the probe printed something that isn't JSON: {e}"),
        ),
    }
}

fn fail(slot: &Slot, error: String) {
    let mut slot = slot.lock().expect("probe slot mutex");
    slot.status = ProbeState::Failed;
    slot.error = Some(error);
}

pub async fn probe_status(
    Path(id): Path<String>,
) -> Result<Json<ProbeStatus>, (StatusCode, Json<Value>)> {
    let slot = jobs().lock().expect("probe jobs mutex").get(&id).cloned();
    let Some(slot) = slot else {
        return Err(err(
            StatusCode::NOT_FOUND,
            "no such probe — it may have already been read, or the server restarted",
        ));
    };
    let status = slot.lock().expect("probe slot mutex").clone();
    // Reap a finished probe on read: the client got the answer, and
    // nothing else will ask for it.
    if status.status != ProbeState::Running {
        jobs().lock().expect("probe jobs mutex").remove(&id);
    }
    Ok(Json(status))
}

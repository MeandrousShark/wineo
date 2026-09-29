use std::path::PathBuf;

use ashpd::desktop::ResponseError;
use ashpd::desktop::file_chooser::SelectedFiles;

pub(crate) async fn pick(title: String, directory: bool) -> Result<Option<PathBuf>, String> {
    let result = SelectedFiles::open_file()
        .title(title.as_str())
        .accept_label("Select")
        .modal(true)
        .multiple(false)
        .directory(directory)
        .send()
        .await
        .and_then(|request| request.response());
    match result {
        Ok(files) => files
            .uris()
            .first()
            .map(|uri| {
                uri.to_file_path()
                    .map_err(|_| "Choose a local file or folder".to_owned())
            })
            .transpose(),
        Err(ashpd::Error::Response(ResponseError::Cancelled)) => Ok(None),
        Err(error) => Err(format!(
            "File chooser unavailable: {error}. Enter a path instead."
        )),
    }
}

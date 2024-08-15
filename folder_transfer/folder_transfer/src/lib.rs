use kinode_process_lib::{
    await_message, call_init, our_capabilities, println, spawn,
    vfs::{create_drive, DirEntry, FileType, VfsAction, VfsRequest, open_file, SeekFrom, open_dir, create_file},
    Address, OnExit, Request, 
};

use files_lib::encryption::{decrypt_data, ENCRYPTED_CHUNK_SIZE};
use files_lib::structs::{WorkerRequest, WorkerStatus};
use files_lib::{read_nested_dir_light};
use std::path::Path;

use base64::{engine::general_purpose, Engine as _};

use serde::{Deserialize, Serialize};


wit_bindgen::generate!({
    path: "target/wit",
    world: "process-v0",
});


#[derive(Serialize, Deserialize, Debug)]
pub enum FolderTransfer {
    // action that triggers request to the target node
    RequestFolderAction {
        node_id: String,
        folder: String,
        encrypt: bool,
    },
    // message that is sent to the target node, requesting them to send the folder
    RequestFolderMessage {
        worker_address: Address,
        folder: String,
        encrypt: bool,
    },
    DecryptFolder,
}

// spawns a worker process for folder transfer (whether it will be for receiving or sending)
fn initialize_worker(
    our: Address,
    current_worker_address: &mut Option<Address>,
) -> anyhow::Result<()> {
    let our_worker = spawn(
        None,
        &format!("{}/pkg/worker.wasm", our.package_id()),
        OnExit::None,
        our_capabilities(),
        vec![],
        false,
    )?;

    // temporarily stores worker address while the worker is alive
    *current_worker_address = Some(Address {
        node: our.node.clone(),
        process: our_worker.clone(),
    });
    Ok(())
}

fn handle_message(
    our: &Address,
    current_worker_address: &mut Option<Address>,
    send_from_path: String,
    send_to_path: String,
    decrypt_to_path: String
) -> anyhow::Result<()> {
    let message = await_message()?;

    if let Ok(request) = serde_json::from_slice::<FolderTransfer>(message.body()) {
        match request {
            // sending request to target node
            FolderTransfer::RequestFolderAction {
                node_id,
                folder,
                encrypt,
            } => {
                println!("RequestFolderAction: node_id: {}", node_id);

                // spin up worker process
                initialize_worker(our.clone(), current_worker_address)?;

                println!("send_to_path: {}", send_to_path[1..].to_string());
                // start receiving data on the worker
                let _worker_request = Request::new()
                    .body(serde_json::to_vec(
                        &WorkerRequest::InitializeReceiverWorker {
                            receive_to_dir: send_to_path[1..].to_string(),
                        },
                    )?)
                    .target(&current_worker_address.clone().unwrap())
                    .send()?;

                // send request to target node
                let request_folder_message =
                    serde_json::to_vec(&FolderTransfer::RequestFolderMessage {
                        worker_address: current_worker_address.clone().unwrap(),
                        folder,
                        encrypt,
                    })?;
                let _request = Request::to(Address::new(node_id.clone(), our.process.clone()))
                    .expects_response(5)
                    .body(request_folder_message)
                    .send()?;
            }
            // received request for folder transfer, sending folder
            FolderTransfer::RequestFolderMessage {
                worker_address,
                folder,
                encrypt,
            } => {
                println!("RequestFolderMessage");

                // spin up worker process
                initialize_worker(our.clone(), current_worker_address)?;

                let sending_dir = format!("{}/{}", send_from_path, folder);
                println!("send_from_path: {}", sending_dir[1..].to_string());
                // start sending data from worker
                let _worker_request = Request::new()
                    .body(serde_json::to_vec(
                        &WorkerRequest::InitializeSenderWorker {
                            target_worker: Some(worker_address.clone()),
                            sending_dir: sending_dir[1..].to_string(),
                            password: if encrypt {
                                Some("some_password".to_string())
                            } else {
                                None
                            },
                        },
                    )?)
                    .target(&current_worker_address.clone().unwrap())
                    .send()?;
            }
            // decrypts content of "send_to" directory, and puts it into "decrypted"
            FolderTransfer::DecryptFolder => {
                println!("DecryptFolder");

                // /folder_transfer:astronaut.os/send_to
                // this is the dir where we transfered the folder in an encrypted form
                let dir_entry: DirEntry = DirEntry {
                    path: send_to_path.to_string(),
                    file_type: FileType::Directory,
                };

                // remove and re-create decrypt_to so it's empty
                let request: VfsRequest = VfsRequest {
                    path: decrypt_to_path.to_string(),
                    action: VfsAction::RemoveDirAll,
                };
                let _message = Request::new()
                    .target(("our", "vfs", "distro", "sys"))
                    .body(serde_json::to_vec(&request)?)
                    .send_and_await_response(5)?;
                let request: VfsRequest = VfsRequest {
                    path: decrypt_to_path.clone(),
                    action: VfsAction::CreateDirAll,
                };
                let _message = Request::new()
                    .target(("our", "vfs", "distro", "sys"))
                    .body(serde_json::to_vec(&request)?)
                    .send_and_await_response(5)?;

                // get all the paths, not content
                let dir = read_nested_dir_light(dir_entry)?;
                // iterate over all files, and decrypt each one
                for path in dir.keys() {
                    let mut active_file = open_file(path, false, Some(5))?;
                    let size = active_file.metadata()?.len;
                    // make sure we start from 0th position every time,
                    // there were some bugs related to files not being closed, so we would start reading from the previous location
                    let _pos = active_file.seek(SeekFrom::Start(0))?;

                    // the path of each encrypted file looks like so:
                    // folder_transfer:astronaut.os/send_to/GAXPVM7g...htLlOiu_E3A
                    let path = Path::new(path);
                    let file_name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_str()
                        .unwrap_or_default()
                        .to_string();
                    // file name decryption
                    //
                    // base64/url_safe encoded encrypted file name -> base64 decoded (still encrypted)
                    // base64 was necessary because of file names not accepting all encrypted chars
                    let decoded_vec = general_purpose::URL_SAFE.decode(&file_name)?;
                    // decoded, encrypted file name -> decrypted file name
                    let decrypted_vec = match decrypt_data(&decoded_vec, "some_password") {
                        Ok(vec) => vec,
                        Err(e) => {
                            println!("couldn't decrypt file name");
                            return Err(anyhow::anyhow!(e));
                        }
                    };
                    let decrypted_path = String::from_utf8(decrypted_vec)
                        .map_err(|e| anyhow::anyhow!("Failed to convert bytes to string: {}", e))?;
                    // get full file_path
                    // one encrypted file name (e.g. q23ewdfvwerv) could be decrypted to a file nested in a folder (e.g. a/b/c/file.md)
                    let file_path = format!("{}{}", decrypt_to_path.to_string(), decrypted_path);
                    // parent path becomes e.g. a/b/c, separated out from a/b/c/file.md
                    let parent_path = Path::new(&file_path)
                        .parent()
                        .and_then(|p| p.to_str())
                        .unwrap_or("")
                        .to_string();
                    // creates nested parent directory (/a/b/c) all the way to the file
                    let request = VfsRequest {
                        path: parent_path.clone(),
                        action: VfsAction::CreateDirAll,
                    };
                    let _message = Request::new()
                        .target(("our", "vfs", "distro", "sys"))
                        .body(serde_json::to_vec(&request)?)
                        .send_and_await_response(5)?;

                    let _dir = open_dir(&parent_path[1..].to_string(), false, Some(5))?;

                    // chunking and decrypting each file
                    //
                    // must be decrypted at specific encrypted chunk size.
                    // encrypted chunk size = chunk size + 44, see files_lib/src/encryption.rs
                    //
                    // potential pitfall in the future is if we modify chunk size,
                    // and try to decrypt at size non corresponding to the size at which it was encrypted.
                    let num_chunks = (size as f64 / ENCRYPTED_CHUNK_SIZE as f64).ceil() as u64;

                    // iterate over encrypted file
                    for i in 0..num_chunks {
                        let offset = i * ENCRYPTED_CHUNK_SIZE;
                        let length = ENCRYPTED_CHUNK_SIZE.min(size - offset); // size=file size
                        let mut buffer = vec![0; length as usize];
                        let _pos = active_file.seek(SeekFrom::Current(0))?;
                        active_file.read_at(&mut buffer)?;

                        // decrypt data with password_hash
                        let decrypted_bytes = match decrypt_data(&buffer, "some_password") {
                            Ok(vec) => vec,
                            Err(_e) => {
                                println!("couldn't decrypt file data");
                                return Err(anyhow::anyhow!("couldn't decrypt file data"));
                            }
                        };

                        let dir = open_dir(&parent_path, false, None)?;

                        // there is an issue with open_file(create: true), so we have to do it manually
                        let entries = dir.read()?;
                        if entries.contains(&DirEntry {
                            path: file_path[1..].to_string(),
                            file_type: FileType::File,
                        }) {
                        } else {
                            let _file = create_file(&file_path, Some(5))?;
                        }

                        let mut file = open_file(&file_path, false, Some(5))?;
                        file.append(&decrypted_bytes)?;
                    }
                }
            }
        }
    }

    // current worker finishing up
    if let Some(worker_address) = current_worker_address {
        if worker_address == message.source() {
            match serde_json::from_slice(&message.body())? {
                WorkerStatus::Done => {
                    *current_worker_address = None;
                    println!("received status: done");
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

call_init!(init);
fn init(our: Address) {
    println!("folder_transfer: begin");

    let send_from_path = create_drive(our.package_id(), "send_from", Some(5)).unwrap();
    let send_to_path = create_drive(our.package_id(), "send_to", Some(5)).unwrap();
    let decrypt_to_path = create_drive(our.package_id(), "decrypt_to", Some(5)).unwrap();
    let mut current_worker_address: Option<Address> = None;

    loop {
        match handle_message(
            &our,
            &mut current_worker_address,
            send_from_path.clone(),
            send_to_path.clone(),
            decrypt_to_path.clone(),
        ) {
            Ok(_) => {}
            Err(e) => println!("Error: {:?}", e),
        }
    }
}

# Folder Transfer Template

Used for transfering a directory from one node to another.

A `worker` is an ephemeral process spawned by the sender and the receiver for the duration of the folder transfer.

## Usage

Boot up 2 nodes, `node.os` and `node2.os`.
Let `home` and `home2` be the home directories of the nodes, respectively.

```
cd folder_transfer
kit bs && kit s -p 8081
```

You will transfer a folder from `node2.os` to `node.os`.
Copy a folder you want to transfer into `home2/vfs/folder_transfer:astronaut.os/send_from`, so that it looks like `home2/vfs/folder_transfer:astronaut.os/send_from/some_folder`.

Then, in `node.os` terminal, run

```
m our@folder_transfer:folder_transfer:astronaut.os '{"RequestFolderAction": {"node_id": "sour-cabbage.os", "folder": "some_folder"}}'
```

Now, in `home/vfs/folder_transfer:astronaut.os/send_to` you should find `some_folder`.

## Explanation

1. `node.os` `folder_transfer` process spawns a receiving worker, and initializes it with `InitializeReceiverWorker`.
2. Then it sends a download request to `node2.os` `folder_transfer` process to receive the folder.
3. `node2.os` `folder_transfer` process spawns a sending worker, and initializes it with `InitializeSenderWorker`.
4. `node2.os` `worker` process sends the folder to `node.os` `worker` process in chunks.
5. Once the transfer is done, each worker sends a `WorkerStatus::Done` to the process that spawned it, and then terminates.

//! The fixture filesystem must reproduce exclusive-creation failures without mutation.

use monty_types::{
    CreateExclusiveCallArgs, ExcType, ExtFunctionResult, FileMode, MontyFileHandle, MontyObject, OsFunctionCall,
};

use crate::{MUTABLE_VFS, dispatch_os_call, get_virtual_file, reset_mutable_vfs};

/// Start each case with no mutable entries and return its typed request.
fn request(path: &str) -> OsFunctionCall {
    reset_mutable_vfs();
    OsFunctionCall::CreateExclusive(CreateExclusiveCallArgs {
        path: path.into(),
        mode: FileMode::Write(false),
        permissions: 0o600,
    })
}

#[test]
fn root_collision_preserves_the_virtual_root() {
    let result = dispatch_os_call(&request("/"));
    let ExtFunctionResult::Error(error) = result else {
        panic!("root must collide");
    };
    assert_eq!(error.exc_type(), ExcType::FileExistsError);
    assert_eq!(error.message(), Some("[Errno 17] File exists: '/'"));
    assert!(get_virtual_file("/").is_none());
    assert!(MUTABLE_VFS.with(|vfs| vfs.borrow().files.is_empty()));
}

#[test]
fn errors_preserve_errno_and_find_the_first_invalid_ancestor() {
    for (path, kind, message) in [
        (
            "/virtual/file.txt",
            ExcType::FileExistsError,
            "[Errno 17] File exists: '/virtual/file.txt'",
        ),
        (
            "/virtual/subdir",
            ExcType::FileExistsError,
            "[Errno 17] File exists: '/virtual/subdir'",
        ),
        (
            "/virtual/absent/file",
            ExcType::FileNotFoundError,
            "[Errno 2] No such file or directory: '/virtual/absent/file'",
        ),
        (
            "/virtual/file.txt/child",
            ExcType::NotADirectoryError,
            "[Errno 20] Not a directory: '/virtual/file.txt/child'",
        ),
        (
            "/virtual/file.txt/child/grandchild",
            ExcType::NotADirectoryError,
            "[Errno 20] Not a directory: '/virtual/file.txt/child/grandchild'",
        ),
    ] {
        let result = dispatch_os_call(&request(path));
        let ExtFunctionResult::Error(error) = result else {
            panic!("expected failure for {path}");
        };
        assert_eq!(error.exc_type(), kind, "{path}");
        assert_eq!(error.message(), Some(message));
        assert!(MUTABLE_VFS.with(|vfs| vfs.borrow().files.is_empty()));
    }
}

#[test]
fn successful_creation_returns_a_handle_and_collisions_preserve_content() {
    let call = request("/virtual/new");
    let ExtFunctionResult::Return(value) = dispatch_os_call(&call) else {
        panic!("expected creation");
    };
    assert_eq!(
        value,
        MontyObject::file_handle(MontyFileHandle {
            path: "/virtual/new".to_owned(),
            mode: FileMode::Write(false),
            position: 0,
        })
    );
    let file = get_virtual_file("/virtual/new").unwrap();
    assert_eq!(file.content, b"");
    assert_eq!(file.mode, 0o600);
    MUTABLE_VFS.with(|vfs| vfs.borrow_mut().files.get_mut("/virtual/new").unwrap().0 = b"sentinel".to_vec());
    let ExtFunctionResult::Error(error) = dispatch_os_call(&call) else {
        panic!("expected collision");
    };
    assert_eq!(error.exc_type(), ExcType::FileExistsError);
    assert_eq!(get_virtual_file("/virtual/new").unwrap().content, b"sentinel");
}

#[test]
fn execute_permissions_are_rejected_before_mutation() {
    let mut call = request("/virtual/new");
    let OsFunctionCall::CreateExclusive(args) = &mut call else {
        unreachable!();
    };
    args.permissions = 0o700;
    let ExtFunctionResult::Error(error) = dispatch_os_call(&call) else {
        panic!("expected invalid permissions");
    };
    assert_eq!(error.exc_type(), ExcType::ValueError);
    assert_eq!(error.message(), Some("execute permission bits are not supported"));
    assert!(get_virtual_file("/virtual/new").is_none());
}

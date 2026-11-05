//! Read-only classification of an otherwise unsafe ACE for an absent account.
//! Never treats an unknown SID as an owner or grants anyone new access.
use std::{
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Networking::ActiveDirectory::*,
    Security::{Authentication::Identity::*, *},
};

/// None means the narrow exception is unavailable; ordinary protected-location
/// validation still works, and every unresolved grant remains rejected.
pub(crate) struct LocalAccounts {
    domain: Option<Vec<usize>>,
}
struct Policy(LSA_HANDLE);
impl Drop for Policy {
    fn drop(&mut self) {
        unsafe {
            LsaClose(self.0);
        }
    }
}
struct PolicyMemory(*mut std::ffi::c_void);
impl Drop for PolicyMemory {
    fn drop(&mut self) {
        unsafe {
            LsaFreeMemory(self.0);
        }
    }
}

fn standalone() -> bool {
    let mut pointer = null_mut();
    if unsafe {
        DsRoleGetPrimaryDomainInformation(null(), DsRolePrimaryDomainInfoBasic, &mut pointer)
    } != ERROR_SUCCESS
        || pointer.is_null()
    {
        return false;
    }
    let role = unsafe { (*pointer.cast::<DSROLE_PRIMARY_DOMAIN_INFO_BASIC>()).MachineRole };
    unsafe {
        DsRoleFreeMemory(pointer.cast());
    }
    // LookupAccountSid can report NONE_MAPPED after a network timeout. Do not
    // make a deleted-account inference on joined/unknown/DC machines.
    eligible_role(role)
}
fn eligible_role(role: DSROLE_MACHINE_ROLE) -> bool {
    role == DsRole_RoleStandaloneWorkstation || role == DsRole_RoleStandaloneServer
}
fn local_domain() -> Option<Vec<usize>> {
    if !standalone() {
        return None;
    }
    let attributes = LSA_OBJECT_ATTRIBUTES {
        Length: size_of::<LSA_OBJECT_ATTRIBUTES>() as u32,
        ..Default::default()
    };
    let mut policy: LSA_HANDLE = 0;
    if unsafe {
        LsaOpenPolicy(
            null(),
            &attributes,
            POLICY_VIEW_LOCAL_INFORMATION as u32,
            &mut policy,
        )
    } != 0
    {
        return None;
    }
    let policy = Policy(policy);
    let mut pointer = null_mut();
    // LOCAL account domain, not the logged-in user's SID or the primary AD
    // domain. No policy setter, privilege adjustment or account enumeration.
    if unsafe {
        LsaQueryInformationPolicy(policy.0, PolicyLocalAccountDomainInformation, &mut pointer)
    } != 0
        || pointer.is_null()
    {
        return None;
    }
    let memory = PolicyMemory(pointer);
    let sid = unsafe { (*memory.0.cast::<POLICY_ACCOUNT_DOMAIN_INFO>()).DomainSid };
    if sid.is_null()
        || unsafe { IsValidSid(sid) } == 0
        || unsafe { *GetSidSubAuthorityCount(sid) } != 4
        || unsafe { *GetSidSubAuthority(sid, 0) } != 21
    {
        return None;
    }
    let size = unsafe { GetLengthSid(sid) };
    if size == 0 || size > SECURITY_MAX_SID_SIZE {
        return None;
    }
    let mut owned = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    if unsafe { CopySid(size, owned.as_mut_ptr().cast(), sid) } == 0 {
        return None;
    }
    Some(owned)
}
impl LocalAccounts {
    pub(crate) fn read() -> Self {
        Self {
            domain: local_domain(),
        }
    }
    #[cfg(test)]
    pub(crate) fn unavailable() -> Self {
        Self { domain: None }
    }

    pub(crate) fn absent_grantee(&self, sid: PSID) -> bool {
        self.absent_with(sid, |sid| {
            let (mut name_size, mut domain_size, mut kind) = (0, 0, 0);
            let found = unsafe {
                LookupAccountSidW(
                    null(),
                    sid,
                    null_mut(),
                    &mut name_size,
                    null_mut(),
                    &mut domain_size,
                    &mut kind,
                )
            };
            found == 0 && unsafe { GetLastError() } == ERROR_NONE_MAPPED
        })
    }
    fn absent_with(&self, sid: PSID, lookup: impl FnOnce(PSID) -> bool) -> bool {
        let Some(local) = &self.domain else {
            return false;
        };
        if sid.is_null()
            || unsafe { IsValidSid(sid) } == 0
            || unsafe { *GetSidSubAuthorityCount(sid) } != 5
            || unsafe { *GetSidSubAuthority(sid, 4) } < 1000
        {
            return false;
        }
        let mut domain = [0usize; 17];
        let mut size = size_of::<[usize; 17]>() as u32;
        if unsafe { GetWindowsAccountDomainSid(sid, domain.as_mut_ptr().cast(), &mut size) } == 0
            || unsafe { EqualSid(domain.as_mut_ptr().cast(), local.as_ptr().cast_mut().cast()) }
                == 0
        {
            return false;
        }
        // On a standalone machine this SID has no current local principal
        // able to exercise the grant. Future administrative account/ACL
        // changes are outside this snapshot and require a fresh check.
        lookup(sid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::Descriptor;
    #[test]
    fn joined_unknown_and_controller_roles_keep_the_strict_policy() {
        assert!(eligible_role(DsRole_RoleStandaloneWorkstation));
        assert!(eligible_role(DsRole_RoleStandaloneServer));
        for role in [
            DsRole_RoleMemberWorkstation,
            DsRole_RoleMemberServer,
            DsRole_RoleBackupDomainController,
            DsRole_RolePrimaryDomainController,
            -1,
            99,
        ] {
            assert!(!eligible_role(role));
        }
    }
    fn owner(sddl: &str) -> Descriptor {
        Descriptor::parse(sddl).unwrap()
    }
    fn sid(descriptor: &Descriptor) -> PSID {
        let (mut sid, mut defaulted) = (null_mut(), 0);
        assert_ne!(
            unsafe { GetSecurityDescriptorOwner(descriptor.raw(), &mut sid, &mut defaulted) },
            0
        );
        sid
    }
    #[test]
    fn namespace_is_checked_before_any_lookup_and_missing_policy_fails_closed() {
        let domain = owner("O:S-1-5-21-111-222-333D:P(A;;FA;;;SY)");
        let raw = sid(&domain);
        let size = unsafe { GetLengthSid(raw) };
        let mut data = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        assert_ne!(unsafe { CopySid(size, data.as_mut_ptr().cast(), raw) }, 0);
        let accounts = LocalAccounts { domain: Some(data) };
        for text in [
            "O:S-1-5-21-444-222-333-1002D:P(A;;FA;;;SY)",
            "O:BUD:P(A;;FA;;;SY)",
            "O:S-1-5-21-111-222-333-500D:P(A;;FA;;;SY)",
        ] {
            let value = owner(text);
            assert!(!accounts.absent_with(sid(&value), |_| panic!(
                "must not query other/builtin domains"
            )));
        }
        let value = owner("O:S-1-5-21-111-222-333-1002D:P(A;;FA;;;SY)");
        assert!(accounts.absent_with(sid(&value), |_| true));
        assert!(!accounts.absent_with(sid(&value), |_| false));
        assert!(!LocalAccounts::unavailable()
            .absent_with(sid(&value), |_| panic!("policy unavailable")));
    }
    #[test]
    fn real_local_policy_never_exempts_the_current_account() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        let mut token = null_mut();
        assert_ne!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) },
            0
        );
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let (mut needed, mut buffer) = (0, vec![0usize; 128]);
        assert_ne!(
            unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    size_of::<usize>() as u32 * buffer.len() as u32,
                    &mut needed,
                )
            },
            0
        );
        let user = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let accounts = LocalAccounts::read();
        if standalone() {
            assert!(
                accounts.domain.is_some(),
                "Read-only LOCAL SAM policy query must succeed on this standalone test machine."
            );
        }
        assert!(!accounts.absent_grantee(user));
    }
}

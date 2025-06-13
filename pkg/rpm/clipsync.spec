Name:           clipsync
Version:        0.1.0
Release:        1%{?dist}
Summary:        Cross-platform clipboard synchronization service
License:        MIT OR Apache-2.0
URL:            https://github.com/lancekrogers/clipsync
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  systemd-rpm-macros
Requires:       systemd

%description
ClipSync is a lightweight service that synchronizes clipboard content
across multiple devices seamlessly. It runs in the background and
automatically syncs text, images, and files between your connected devices.

Features:
- Real-time clipboard synchronization
- End-to-end encryption
- Multi-device support
- Minimal resource usage

%prep
%setup -q

%build
# Binary is pre-built

%install
# Install binary
install -D -m 755 clipsync %{buildroot}%{_bindir}/clipsync

# Install systemd service
install -D -m 644 scripts/clipsync.service %{buildroot}%{_userunitdir}/clipsync.service

# Create directories
install -d -m 755 %{buildroot}%{_sysconfdir}/clipsync
install -d -m 755 %{buildroot}%{_localstatedir}/lib/clipsync

%post
%systemd_user_post clipsync.service

%preun
%systemd_user_preun clipsync.service

%postun
%systemd_user_postun_with_restart clipsync.service

%files
%license LICENSE-MIT LICENSE-APACHE
%doc README.md
%{_bindir}/clipsync
%{_userunitdir}/clipsync.service
%dir %{_sysconfdir}/clipsync
%dir %{_localstatedir}/lib/clipsync

%changelog
* Wed Nov 01 2024 ClipSync Team <support@clipsync.com> - 0.1.0-1
- Initial release of ClipSync
- Cross-platform clipboard synchronization
- Systemd service integration
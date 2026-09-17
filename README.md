# CamBridge Rust (Windows 11 x64)

Rust 송신·수신 데스크톱 앱과 Windows Media Foundation 가상 카메라 DLL입니다.
기존 GitHub 원본에서 개발한 2.0 시험 버전이며, 실제 장비 검증 범위는 아래에 구분합니다.

## 사용 방법

1. 두 PC에 같은 Rust 빌드를 압축 해제합니다.
2. 가상 카메라가 필요한 PC에서 `Install-CamBridge.ps1`을 실행합니다. 관리자 권한으로 DLL 등록 및 Private LAN UDP 방화벽 설정을 합니다.
3. 수신 PC에서 `CamBridge.exe`를 열고 **수신 시작**을 누릅니다.
4. 송신 PC에서 입력 장치와 정확한 입력 모드를 선택하고 **수신 IP**에 수신 PC 주소를 입력한 뒤 **송신 시작**을 누릅니다.
5. 수신 PC에서 **가상 카메라 / OBS 호환 모드**를 켜고 OBS의 비디오 캡처 장치로 **CamBridge**를 선택합니다.
6. 창을 닫으면 트레이로 숨겨집니다. 트레이 아이콘 우클릭으로 설정을 열거나 종료합니다. **Windows 시작 시 실행**은 현재 사용자 시작 프로그램으로 트레이 앱을 등록합니다.

앱의 움직이는 **테스트 패턴 (실제 장치 아님)**으로 카메라 없이 연결을 확인할 수 있습니다.
같은 PC의 두 앱으로 시험할 때 수신 IP는 `127.0.0.1`입니다. 실제 가상 카메라/파이프 생산자는 PC당 하나만 켜세요.

OBS는 송신의 필수 구성 요소가 아닙니다. OBS 호환 모드에서는 CamBridge가 물리 장치를 열고,
OBS가 CamBridge 가상 카메라를 사용합니다. 다른 앱이 이미 독점 점유한 물리 장치를 강제로 공유하지는 못합니다.

## 입력 장치와 모드

- Media Foundation 비디오 캡처 소스로 노출되는 웹캠과 캡처보드가 대상입니다.
- 제조사 전용 SDK만 제공하는 장치는 지원하지 않습니다.
- 이름과 심볼릭 링크로 장치를 식별합니다. 웹캠/캡처보드 종류에 대한 이름 기반 힌트는 확정 분류가 아니므로 UI에는 장치 이름을 표시합니다.
- 드라이버가 제공하는 해상도, FPS 분수, 입력 포맷의 **실제 튜플**을 열거합니다. 서로 다른 모드에서 4K와 60fps를 조합하지 않습니다.
- 체크 표시는 `SetCurrentMediaType` 설정을 드라이버가 수락했다는 뜻입니다. 실제 프레임 수신은 송신 시작 시 확인합니다. 거부된 모드는 송신 시작이 차단됩니다.
- 출력 크기는 입력과 동일/가로세로 1/2/1/4, 송신 FPS는 입력과 동일/최대 30/최대 15이며 입력보다 높게 설정하지 않습니다.
- 장치 오류, 스트림 종료, 입력 형식 변경 및 3초 동안 프레임 응답이 없으면 상태를 표시하고 같은 모드로 다시 연결합니다.
- 일부 캡처보드는 HDMI 신호가 없어도 검은 프레임이나 자체 안내 화면을 계속 보냅니다. 표준 MF 정보만으로 그 상태를 실제 영상과 확실하게 구별할 수 없으며, 별도 신호 감지 지원을 보장하지 않습니다.

## 전송 코덱과 로컬 출력

입력 포맷과 전송 코덱은 별도 항목입니다.

| 전송 | 동작 |
| --- | --- |
| JPEG/MJPEG | Rust CPU JPEG 인코딩/디코딩. MJPEG 입력에서 크기가 같으면 원본 JPEG를 재인코딩 없이 전달합니다. |
| XRGB8888 | BGRX 바이트 순서의 원본 전송. 자동 기본값은 JPEG이며 UI에 원본 대역폭을 표시합니다. 4K60은 헤더 제외 약 15.93 Gbit/s입니다. |
| H.264 / HEVC / AV1 | Windows MFT 인코더/디코더 연동. **코덱 검사**로 시험 프레임 출력에 성공한 인코더만 표시합니다. 설치된 코덱과 GPU 드라이버에 따라 목록이 다릅니다. |
| 압축 입력과 동일한 전송 | 장치 비트스트림 원본 옵션을 제공합니다. 원본 크기/FPS를 유지해야 하며, 네트워크용 디코딩·재인코딩을 생략합니다. 로컬 프리뷰/OBS를 위한 디코딩은 별도입니다. |

CPU/GPU 인코더는 실제 선택 모드에서 초기화를 다시 확인합니다. 하드웨어 MFT 중 시스템 메모리 입출력을 지원하는 구현을 대상으로 하며, 모든 GPU의 모든 코덱 지원을 보장하지 않습니다.
JPEG에는 품질 설정, MFT에는 비트레이트 설정이 적용됩니다. 저지연 모드는 MFT가 받아들이는 경우 활성화합니다.
현재 지연 모드를 선택하는 별도 UI는 없습니다. 원본 전달에는 품질/비트레이트 설정이 적용되지 않습니다.

수신자의 정상 응답이 없거나 3초 동안 응답이 끊기면 네트워크 인코딩을 중지합니다.
로컬 프리뷰와 OBS용 로컬 가상 카메라 출력은 유지됩니다. 수동 **코덱 검사**는 짧은 시험 인코딩을 수행합니다.

프로토콜 v2는 코덱, 크기, FPS, 타임스탬프, 키프레임 및 코덱 설정 정보를 전달합니다.
수신 측은 프레임 손실 뒤 키프레임을 기다리고 요청합니다. 송신 인코더에는 주기적인 키프레임도 요청합니다.
물리 장치의 압축 원본에는 장치 자체 키프레임 주기를 따릅니다.

기존 C# JPEG 프로토콜과 Rust v2는 호환되지 않으므로 두 PC에 같은 Rust 버전을 사용하세요.
프레임 상한은 128 MiB, UDP 패킷 상한은 1200바이트, 재조립 만료는 750ms입니다.
UDP에는 인증·암호화·재전송이 없으며 Private LAN 전용입니다. 큰 원본 프레임은 패킷 손실에 민감합니다.

새 가상 카메라 DLL은 가변 크기 로컬 파이프 v2와 720p/1080p/4K, 30/60fps 출력 모드를 제공합니다.
OBS가 선택한 가상 출력과 수신 영상 크기가 다르면 DLL에서 크기를 변환하며, 실제 입력 FPS보다 높은 가상 출력은 새 프레임을 만들어 내지 않습니다.
기존 DLL과의 호환 파이프 v1도 제공하며 이 경로는 1080p30으로 제한됩니다.
가상 카메라를 4K로 사용하려면 새 DLL 빌드가 필요합니다.

## 빌드

Rust stable, Visual Studio 2022 C++ 도구와 Windows 11 SDK, NuGet이 필요합니다.

```powershell
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
nuget restore VirtualCameraMediaSource/packages.config -PackagesDirectory packages
msbuild VirtualCameraMediaSource/VirtualCameraMediaSource.vcxproj /m /p:Configuration=Release /p:Platform=x64 /p:WindowsTargetPlatformVersion=10.0 /p:SolutionDir="$pwd\\" /p:OutDir="$pwd\\native-build\\"
```

GitHub Actions `Windows build and package`는 Rust 검사와 네이티브 DLL 빌드 후 설치용 ZIP을 아티팩트로 생성합니다.
MSVC 빌드는 LLVM 런타임 DLL이 필요 없습니다. gnullvm 빌드에는 같은 폴더의 `libunwind.dll`이 필요합니다.

진단 명령: `--list-devices`, `--probe-codecs`, `--protocol-version`, `--xrgb-bandwidth 3840 2160 60`.

## 검증 기록과 남은 제한

개발 PC에서 Rust 빌드와 15개 자동 테스트를 통과했습니다.
실제 UDP 소켓의 테스트 영상 송수신, JPEG 복원, 수신자 부재 시 인코딩 0 및 프리뷰 유지,
33MB 4K XRGB 프레임의 역순 재조립, 잘못된 패킷 거부를 확인했습니다.

장치 검색에서 `GC21 Video`가 발견됐지만 앞선 장치 활성화는 `0x80070005`로 거부됐습니다.
현재 실제 카메라/GPU 검증 실행도 실행 환경의 승인 검토에 의해 차단되어 다음 항목은 **실장비 미검증**입니다.

- 1080p30 웹캠과 4K60 캡처보드의 실제 모드/프레임 수신 및 지속 성능
- H.264/HEVC/AV1 실제 장치 원본 전달과 각 CPU/GPU MFT 조합의 송수신
- 장치 분리, HDMI 신호 중단/변경/복구
- OBS에서 새 DLL의 4K/60fps 출력 및 장시간 안정성
- 실제 카메라의 수신자 유무에 따른 CPU 사용률 비교 (인코딩 호출 생략은 테스트됨)

GitHub Actions Windows 빌드에서 Rust 검사·15개 테스트·Release EXE·수정된 네이티브 DLL 빌드와 설치 ZIP 생성을 통과했습니다.
빌드 및 자동 테스트 통과는 위 실장비 검증을 대신하지 않습니다. 이 배포본은 2.0 시험 버전입니다.

네이티브 가상 카메라는 MIT 라이선스의 Microsoft Windows-Camera 샘플에 기반하며 `VirtualCameraMediaSource/LICENSE.microsoft`를 포함합니다.

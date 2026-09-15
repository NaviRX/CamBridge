# CamBridge (Windows 11 x64)

> Rust 2.0 migration is in progress on top of the original C# implementation. The existing C# app and native Media Foundation virtual-camera DLL remain the deployable path until the Rust sender/receiver UI reaches feature parity. Do not ship the Rust CLI as a replacement yet.

물리 카메라를 Windows Media Foundation으로 읽어 JPEG/UDP로 전송하고, 다른 PC에서 Windows Media Foundation 가상 카메라로 출력하는 트레이 앱입니다. 

## 설치 및 실행

1. 양쪽 PC에서 배포 ZIP을 압축 해제합니다. 관리자 PowerShell에서 `Install-CamBridge.ps1`을 실행합니다. 설치는 앱과 네이티브 미디어 소스를 Program Files에 복사하고 COM 소스와 Private LAN용 UDP 45831/45832 방화벽 규칙을 등록합니다. 네트워크 프로필은 Private이어야 합니다.

2. 카메라가 연결된 PC에서 CamBridge를 열고 `송신`, 실제 카메라, 1920×1080 30fps 캡처 형식을 선택해 시작합니다. 화면의 `이 PC 송신 IP`를 확인합니다. FPS와 JPEG 품질을 조절할 수 있습니다.

3. 카메라 영상을 사용할 PC에서 CamBridge를 열고 `수신`을 선택합니다. `수신 IP`에 2단계에서 확인한 주소를 입력하고 시작합니다. 상태에 가상 카메라 사용 가능 메시지가 나타나는지 확인합니다.

4. OBS/Discord/브라우저 카메라 목록에서 `CamBridge (Windows 가상 카메라)`를 선택합니다.

창을 닫으면 트레이에서 계속 실행됩니다. 트레이 메뉴에서 시작/중지·열기·종료가 가능하며 Windows 자동 시작을 켤 수 있습니다. 카메라 연결이 끊기면 2초 간격으로 재연결합니다.

같은 물리 카메라를 OBS와 CamBridge가 동시에 열지 못한다면 카메라가 연결된 PC에서 `OBS 호환 모드 (로컬 가상 카메라)`를 켭니다. 이때 CamBridge만 물리 카메라를 열고 OBS 비디오 캡처 장치에서 `CamBridge (Windows 가상 카메라)`를 선택합니다. OBS 플러그인이나 OBS 실행은 CamBridge 송신에 필요하지 않습니다.

## 검증 범위

- Elgato Facecam Pro 1920×1080 30fps MJPEG 캡처를 확인했습니다.
- 프리뷰의 상하 반전을 수정했고, 실제 카메라 화면에서 정상 방향을 확인했습니다. 송신 IP에는 사설 LAN의 실제 유선/Wi-Fi 주소만 표시하며 수신 IP 입력도 사설 IPv4로 제한합니다.
- 물리 카메라 프레임을 로컬 프레임 파이프로 전달하고 CamBridge Windows 가상 카메라에서 다시 읽는 시험에 성공했습니다.
- 두 Media Foundation 클라이언트의 동일 Facecam 동시 캡처는 장치 선점 오류가 발생했습니다. OBS와 함께 쓸 카메라의 다중 접근 여부는 장치별로 시험해야 합니다.
- 두 PC를 연결한 후 프레임 송신이 시작되는 것을 확인했습니다. 수신 측의 최종 방향, 화질·지연·프레임 손실, OBS/Discord 노출은 추가 검증이 필요합니다.
- JPEG/UDP는 인증과 암호화가 없습니다. 신뢰할 수 있는 Private LAN에서만 사용하세요.

네이티브 가상 카메라 미디어 소스는 MIT 라이선스인 Microsoft Windows-Camera 샘플을 기반으로 하며 `VirtualCameraMediaSource/LICENSE.microsoft`를 포함합니다. Windows 11 빌드 22000 이상이 필요합니다.

## Rust 2.0 구현 현황

Rust 코드는 `src/`에 있으며 다음 기반 기능을 구현합니다.

- Media Foundation 비디오 캡처 범주의 웹캠·캡처보드 통합 검색
- 각 장치가 광고한 `(해상도, FPS 분수, 입력 포맷)` 튜플을 그대로 열거하며 조합을 합성하지 않음
- `IMFSourceReader::SetCurrentMediaType`으로 각 네이티브 모드의 열기 수락 여부 확인
- MJPEG/H.264/H.265/AV1 압축 입력과 XRGB8888 입력의 무재인코딩 패스스루 판정
- NV12/YUY2/XRGB 및 압축 입력에 필요한 변환·디코딩·인코딩 경로의 사전 검증
- 수신자가 없을 때 네트워크 인코딩을 생략하는 송신 작업 상태
- 코덱·크기·FPS·키프레임·설정 정보·100ns 타임스탬프를 포함한 프로토콜 v2
- 128MiB 프레임, 최대 6개 동시 프레임, 750ms 만료를 적용한 제한형 UDP 재조립
- XRGB8888 예상 대역폭 계산(4K60은 약 15.93Gbit/s) 및 기본값 선택 금지 근거
- 장치 분리·신호 없음·지수 백오프 재연결 상태 모델

현재 Rust CLI 명령:

```powershell
cambridge.exe --list-devices
cambridge.exe --protocol-version
cambridge.exe --xrgb-bandwidth 3840 2160 60
```

### 아직 배포 기능이 아닌 항목

- Rust GUI·트레이·자동 시작 설정 화면
- Rust 캡처 루프와 로컬 프리뷰
- Media Foundation MFT 하드웨어 인코더/디코더의 런타임 열거와 실행
- H.264/H.265/AV1 코덱 설정 레코드와 키프레임 요청을 포함한 실제 MFT 송수신
- Rust 수신 프레임을 기존 가상 카메라 파이프로 전달하는 연동

이 항목들이 완료되기 전에는 UI에 코덱이나 하드웨어 인코더를 선택 가능한 것으로 표시해서는 안 됩니다. 현재 배포본은 기존 JPEG/C# 경로를 사용합니다.

## 지원 장치 범위

Windows Media Foundation의 `MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID` 범주로 노출되는 UVC 웹캠과 캡처보드가 대상입니다. Media Foundation에 비디오 캡처 소스로 나타나지 않거나 제조사 전용 SDK가 필요한 장치는 지원 대상이 아닙니다. Windows에는 웹캠과 캡처보드를 구분하는 표준 속성이 없으므로 장치 종류 표시는 이름·심볼릭 링크 기반의 보수적인 힌트이며, 식별이 확실하지 않으면 `비디오 캡처 장치`로 표시합니다.

HDMI 신호가 없는 상태와 장치 분리는 서로 다른 상태로 표시해야 합니다. 신호 또는 장치가 돌아오면 선택했던 정확한 네이티브 모드로 재시도하며, 존재하지 않는 대체 모드를 만들어 자동 선택하지 않습니다.

## Rust 개발 및 검증

```powershell
cargo test
cargo run -- --list-devices
```

순수 로직 테스트는 정확한 모드 제한, 1080p30 상한, 비트스트림 패스스루, 수신자 부재 시 인코딩 생략, 프로토콜 버전 오류, 4K 프레임 재조립 및 XRGB 대역폭 계산을 검증합니다.

이 저장소가 테스트된 개발 PC에서는 Media Foundation이 `GC21 Video`를 검색했지만 장치 활성화가 `0x80070005`로 거부되어 네이티브 모드와 HDMI 신호 복구를 실장비로 검증하지 못했습니다. 1080p30 및 4K60 장비의 실측 검증은 구현 완료와 별개로 남아 있습니다.

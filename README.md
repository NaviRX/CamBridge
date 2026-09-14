# CamBridge (Windows 11 x64)

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

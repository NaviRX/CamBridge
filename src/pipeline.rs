use crate::capability::{
    CaptureMode, CodecCapability, PixelFormat, TransportCodec, available_encoders,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelinePlan {
    Passthrough,
    RawPassthrough,
    DecodeAndEncode,
    ConvertAndEncode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    EncoderUnavailable(TransportCodec),
    DecoderUnavailable(PixelFormat),
    RawFormatMismatch,
}

pub fn plan_pipeline(
    input: &CaptureMode,
    transport: TransportCodec,
    capabilities: &[CodecCapability],
) -> Result<PipelinePlan, PlanError> {
    if TransportCodec::from_input(input.format) == Some(transport) {
        return Ok(if input.format == PixelFormat::Xrgb8888 {
            PipelinePlan::RawPassthrough
        } else {
            PipelinePlan::Passthrough
        });
    }
    if transport == TransportCodec::Xrgb8888 {
        return Err(PlanError::RawFormatMismatch);
    }
    if available_encoders(capabilities, transport).is_empty() {
        return Err(PlanError::EncoderUnavailable(transport));
    }
    if input.format.compressed()
        && !capabilities
            .iter()
            .any(|c| c.can_decode && TransportCodec::from_input(input.format) == Some(c.codec))
    {
        return Err(PlanError::DecoderUnavailable(input.format));
    }
    Ok(if input.format.compressed() {
        PipelinePlan::DecodeAndEncode
    } else {
        PipelinePlan::ConvertAndEncode
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderWork {
    CapturePreviewAndLocalCameraOnly,
    CaptureAndTransmit,
}

pub fn sender_work(has_live_receiver: bool) -> SenderWork {
    if has_live_receiver {
        SenderWork::CaptureAndTransmit
    } else {
        SenderWork::CapturePreviewAndLocalCameraOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::FrameRate;

    fn mode(format: PixelFormat) -> CaptureMode {
        CaptureMode {
            width: 3840,
            height: 2160,
            fps: FrameRate::new(60, 1),
            format,
            native_type_index: 0,
            verified_openable: true,
        }
    }

    #[test]
    fn matching_compressed_input_is_passed_through_without_encoder() {
        assert_eq!(
            plan_pipeline(&mode(PixelFormat::H264), TransportCodec::H264, &[]),
            Ok(PipelinePlan::Passthrough)
        );
    }

    #[test]
    fn no_receiver_skips_network_encoding() {
        assert_eq!(
            sender_work(false),
            SenderWork::CapturePreviewAndLocalCameraOnly
        );
    }
}

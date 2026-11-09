//! Before-original camera distance with a pinned managed field. A game setting changed while enabled becomes
//! the new restoration baseline. The shared lifecycle closes/drains/restores before releasing the pin; observing
//! a replacement camera rebinds the same wanted setting. Immediate ApplyZoom is a best-effort main-thread nicety,
//! never a shutdown prerequisite: control-thread unload restores only managed scalar storage.
use super::registry::*;
use crate::game::*;
use il2cpp_runtime::{AttachedThread, InvocationContext, InvocationOutcome, ReturnValue};

pub(crate) const FEATURE: HookFeature = HookFeature {
    controls: &[GameplayHook::CameraDistance],
    behavior: Behavior::Camera,
    targets: Targets::Resolve(targets),
};
const SPEC: MethodSpec = MethodSpec {
    class: c"CameraManager",
    method: c"ApplyZoom",
    instance: true,
    params: &[],
    returns: "System.Void",
};
impl RuntimeAccess {
    unsafe fn zoom_camera<'a, 'r>(&self, attached: &'a AttachedThread<'r>) -> Result<Option<Root<'a, 'r>>, GameError> {
        // SAFETY: admitted main thread; root the static reference before inspecting managed/native liveness.
        unsafe {
            let owner = self.class(&self.images()?, c"", c"CameraManager")?;
            let Some(camera) = self.static_object(attached, owner, c"instance", owner)? else {
                return Ok(None);
            };
            if self.primitive::<usize>(&camera, c"m_CachedPtr")? == 0 {
                return Ok(None);
            }
            Ok(Some(camera))
        }
    }
    pub(crate) unsafe fn bind_camera(&self, attached: &AttachedThread<'_>) -> Result<ScalarBinding, GameError> {
        // SAFETY: rooted current camera; validate the mutable field and pin its managed storage before publishing.
        unsafe {
            let camera = self.zoom_camera(attached)?.ok_or(GameError::NotReady("camera unavailable"))?;
            let owner = self.class(&self.images()?, c"", c"CameraManager")?;
            let field = self
                .mutable_field(owner, c"zoomDefault", c"Single", raw::type_code::R4)
                .map_err(|e| unsupported(e.to_string()))?;
            let offset = self.hook_field(owner, c"zoomDefault", "System.Single", 4)?;
            let baseline = self.single_bits(&camera, field.as_ptr())?;
            let online = self.hook_online_storage(&self.images()?)?.as_ptr() as usize;
            ScalarBinding::pin(self, target(&camera)?.as_ptr() as usize, offset, baseline, online)
        }
    }
    pub(crate) unsafe fn current_camera(&self, attached: &AttachedThread<'_>) -> Result<usize, GameError> {
        // SAFETY: main-thread read; the temporary root spans the identity read.
        unsafe { self.zoom_camera(attached)?.as_ref().map_or(Ok(0), |root| Ok(target(root)?.as_ptr() as usize)) }
    }
    pub(crate) unsafe fn refresh_camera(&self, attached: &AttachedThread<'_>) -> Result<(), GameError> {
        // SAFETY: admitted main thread and rooted live camera; exact validated void method, no arguments or retries.
        unsafe {
            let Some(camera) = self.zoom_camera(attached)? else { return Ok(()) };
            let owner = self.class(&self.images()?, c"", c"CameraManager")?;
            let method = self.hook_method_contract(owner, &SPEC)?;
            let context = InvocationContext::new(attached);
            let call = context.bind(NonNull::new(method.cast_mut()).unwrap()).map_err(|e| unsupported(e.to_string()))?;
            match call.invoke(Some(&camera), &mut []) {
                Ok(InvocationOutcome::Returned(ReturnValue::Void)) => Ok(()),
                _ => Err(unsupported("camera zoom refresh")),
            }
        }
    }
}
unsafe fn targets(access: &RuntimeAccess, module: NonNull<u8>, size: usize) -> Result<Vec<GameplayHookTarget>, GameError> {
    // SAFETY: attached metadata, retained method; binding validates and pins the receiver separately.
    unsafe { Ok(vec![access.hook_method(&SPEC, module, size)?.scalar(false)]) }
}

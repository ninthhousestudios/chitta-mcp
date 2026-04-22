import onnxruntime as ort

sess = ort.InferenceSession(
    "/root/.cache/chitta/bge-m3-onnx/bge_m3_model.onnx",
    providers=["CUDAExecutionProvider", "CPUExecutionProvider"],
)

for node in sess.get_providers():
    print("Active provider:", node)

print()
for n in sess.get_provider_options():
    print(n)

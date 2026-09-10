// GPUI が描いた PNG を macOS Vision で読み取り、元の URL と一致するか検証する。
import Foundation
import Vision
import ImageIO
let imageURL = URL(fileURLWithPath: CommandLine.arguments[1])
let source = CGImageSourceCreateWithURL(imageURL as CFURL, nil)!
let image = CGImageSourceCreateImageAtIndex(source, 0, nil)!
let request = VNDetectBarcodesRequest()
request.symbologies = [.qr]
try VNImageRequestHandler(cgImage: image).perform([request])
let payload = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[2]))) as! [String: Any]
guard request.results?.contains(where: { $0.payloadStringValue == payload["url"] as? String }) == true else {
    print("QR decode failed")
    exit(1)
}
print("QR decoded and matches original URL")
let text = VNRecognizeTextRequest()
text.recognitionLanguages = ["ja-JP", "en-US"]
try VNImageRequestHandler(cgImage: image).perform([text])
print("UI text:", (text.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(separator: " / "))

fn main() {
    let json = r##"{
      "text": "Hello world of QR codes!",
      "radius": "0.5",
      "ecLevel": "H",
      "fill": "#0064a8",
      "background": null,
      "size": "250"
    }"##;
    let opts: qr_creator::Options = serde_json::from_str(json).unwrap();
    let img = opts.render_image().unwrap();
    img.save("demo_qr.png").unwrap();
    println!("wrote demo_qr.png {}x{}", img.width(), img.height());
}

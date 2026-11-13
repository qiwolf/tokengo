//! Audio quantities are media duration, never HTTP elapsed time or token counts.
use serde_json::Value;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

fn seconds_ms(seconds:f64)->Option<i64> {
    let ms=(seconds*1000.0).ceil();
    (seconds.is_finite()&&ms>0.0&&ms<(i64::MAX as f64)).then_some(ms as i64)
}
pub fn reported_duration_ms(body:&[u8])->Option<i64> {
    let v:Value=serde_json::from_slice(body).ok()?;
    let result=[v.pointer("/usage/audio_seconds"),v.pointer("/usage/seconds"),v.pointer("/usage/duration"),v.get("duration")]
        .into_iter().flatten().filter_map(Value::as_f64).find_map(seconds_ms);
    result
}
pub fn wav_duration_ms(data:&[u8])->Option<i64> {
    if data.len()<12||&data[..4]!=b"RIFF"||&data[8..12]!=b"WAVE" {return None;}
    let riff=u32::from_le_bytes(data[4..8].try_into().ok()?) as usize;
    let end=riff.checked_add(8)?;if end>data.len() {return None;}
    let mut position=12usize;let mut rate=None;let mut sample_bytes=None;
    while position.checked_add(8)?<=end {
        let n=u32::from_le_bytes(data[position+4..position+8].try_into().ok()?) as usize;
        let start=position+8;let stop=start.checked_add(n)?;if stop>end{return None;}
        if &data[position..position+4]==b"fmt "&&n>=16 {
            let format=u16::from_le_bytes(data[start..start+2].try_into().ok()?);
            let channels=u16::from_le_bytes(data[start+2..start+4].try_into().ok()?) as u64;
            let hz=u32::from_le_bytes(data[start+4..start+8].try_into().ok()?) as u64;
            let bytes=u32::from_le_bytes(data[start+8..start+12].try_into().ok()?) as u64;
            let align=u16::from_le_bytes(data[start+12..start+14].try_into().ok()?) as u64;
            let bits=u16::from_le_bytes(data[start+14..start+16].try_into().ok()?) as u64;
            if !matches!(format,1|3)||channels==0||hz==0||bits==0||bits%8!=0||align!=channels.checked_mul(bits/8)?||bytes!=hz.checked_mul(align)? {return None;}
            rate=Some((bytes,align));
        }
        if &data[position..position+4]==b"data" {if sample_bytes.is_some(){return None;}sample_bytes=Some(n as u64);}
        position=stop.checked_add(n%2)?;
    }
    let (bytes,align)=rate?;let n=sample_bytes?;if n==0||n%align!=0{return None;}
    i64::try_from(n.checked_mul(1000)?.checked_add(bytes-1)?/bytes).ok()
}
/// MPEG Layer III complete frames, including VBR. No guessed bitrate/file-size ratio.
pub fn mp3_duration_ms(data:&[u8])->Option<i64> {
    let mut position=0usize;
    if data.starts_with(b"ID3") {
        if data.len()<10||data[6..10].iter().any(|b|b&128!=0){return None;}
        let size=data[6..10].iter().fold(0usize,|n,b|(n<<7)|usize::from(*b));
        position=10usize.checked_add(size)?.checked_add(if data[5]&0x10!=0{10}else{0})?;
    }
    let mut ticks=0u64;let mut frames=0u64;
    while position+4<=data.len() {
        if data.len()-position==128&&&data[position..position+3]==b"TAG" {position=data.len();break;}
        let h=u32::from_be_bytes(data[position..position+4].try_into().ok()?);
        if h&0xffe00000!=0xffe00000{return None;}
        let version=(h>>19)&3;let layer=(h>>17)&3;let bitrate=(h>>12)&15;let sample=(h>>10)&3;let padding=(h>>9)&1;
        if version==1||layer!=1||matches!(bitrate,0|15)||sample==3{return None;}
        let high=version==3;
        let kbps=if high {[0,32,40,48,56,64,80,96,112,128,160,192,224,256,320,0][bitrate as usize]}else{[0,8,16,24,32,40,48,56,64,80,96,112,128,144,160,0][bitrate as usize]} as u64;
        let hz=[44100u64,48000,32000][sample as usize]/if high{1}else if version==2{2}else{4};
        let length=((if high{144000}else{72000})*kbps/hz+u64::from(padding)) as usize;
        if length<4||position.checked_add(length)?>data.len(){return None;}
        let samples=if high{1152u64}else{576};
        ticks=ticks.checked_add(samples*14_112_000/hz)?;
        position+=length;frames+=1;
    }
    if frames==0||position!=data.len(){return None;}
    i64::try_from(ticks.checked_mul(1000)?.checked_add(14_111_999)?/14_112_000).ok()
}
pub async fn input_duration_ms(data:&[u8])->Option<i64> {
    if let Some(n)=wav_duration_ms(data).or_else(||mp3_duration_ms(data)){return Some(n);}
    // Optional decoder for FLAC, AAC, Ogg and other containers. Missing decoder
    // is unknown usage; neither the filename nor a client duration is trusted.
    tokio::time::timeout(std::time::Duration::from_secs(10),async {
        let program=std::env::var("TOKENGO_FFPROBE").unwrap_or_else(|_|"ffprobe".into());
        let mut child=tokio::process::Command::new(program).args(["-v","error","-protocol_whitelist","pipe","-show_entries","format=duration","-of","json","-i","pipe:0"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn().ok()?;
        child.stdin.take()?.write_all(data).await.ok()?;
        let output=child.wait_with_output().await.ok()?;if !output.status.success()||output.stdout.len()>8192{return None;}
        let v:Value=serde_json::from_slice(&output.stdout).ok()?;
        seconds_ms(v.pointer("/format/duration")?.as_str()?.parse().ok()?)
    }).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn wav(milliseconds:u32)->Vec<u8> {
        let n=milliseconds*32;let mut data=Vec::new();data.extend_from_slice(b"RIFF");data.extend_from_slice(&(36+n).to_le_bytes());data.extend_from_slice(b"WAVEfmt ");data.extend_from_slice(&16u32.to_le_bytes());data.extend_from_slice(&1u16.to_le_bytes());data.extend_from_slice(&1u16.to_le_bytes());data.extend_from_slice(&16000u32.to_le_bytes());data.extend_from_slice(&32000u32.to_le_bytes());data.extend_from_slice(&2u16.to_le_bytes());data.extend_from_slice(&16u16.to_le_bytes());data.extend_from_slice(b"data");data.extend_from_slice(&n.to_le_bytes());data.resize(44+n as usize,0);data
    }
    #[test] fn wav_uses_media_bytes_and_validates_truncation(){assert_eq!(wav_duration_ms(&wav(1250)),Some(1250));let mut broken=wav(1000);broken.pop();assert_eq!(wav_duration_ms(&broken),None);let mut broken=wav(1000);broken[28..32].copy_from_slice(&1u32.to_le_bytes());assert_eq!(wav_duration_ms(&broken),None);}
    #[test] fn provider_duration_never_comes_from_tokens_or_latency(){assert_eq!(reported_duration_ms(br#"{"duration":1.2341,"usage":{"total_tokens":999999}}"#),Some(1235));assert_eq!(reported_duration_ms(br#"{"duration":-1,"usage":{"total_tokens":100},"latency_ms":2000}"#),None);assert_eq!(reported_duration_ms(br#"{"usage":{"audio_seconds":2.5}}"#),Some(2500));}
    #[test] fn mp3_counts_complete_vbr_frames(){let mut data=vec![0u8;417+522];data[0..4].copy_from_slice(&[0xff,0xfb,0x90,0x00]);data[417..421].copy_from_slice(&[0xff,0xfb,0xa0,0x00]);assert_eq!(mp3_duration_ms(&data),Some(53));data.pop();assert_eq!(mp3_duration_ms(&data),None);}
}

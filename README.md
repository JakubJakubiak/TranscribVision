# TranscribVision

- Uses [`OpenAI Whisper`](https://openai.com/research/whisper) and [`Stable-TS`](https://github.com/jianfch/stable-ts) for **extremely accurate transcription**.



![photo](https://raw.githubusercontent.com/JakubJakubiak/TranscribVision/main/images/Screen.png?token=GHSAT0AAAAAACE7LG57O54RX6OJLZUJPQOYZHTGKPQ)



## Setup
```
pip install -U stable-ts
```

To install the latest commit:
```
pip install -U git+https://github.com/jianfch/stable-ts.git
```

## Usage
The following is a list of CLI usages each followed by a corresponding Python usage (if there is one). 

### Transcribe
```commandline
stable-ts audio.mp3 -o audio.srt
```

## Acknowledgments
Includes slight modification of the original work: [Whisper](https://github.com/openai/whisper)

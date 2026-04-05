package main

import (
	"bytes"
	"context"
	"encoding/binary"
	"errors"
	"flag"
	"fmt"
	"os"
	"strings"
	"time"

	speechmesh "speechmesh-go"
)

func main() {
	var (
		url      = flag.String("url", "ws://127.0.0.1:8765/ws", "SpeechMesh WebSocket URL")
		wavPath  = flag.String("wav", "", "path to mono 16k PCM S16LE wav file")
		language = flag.String("language", "en-US", "recognition language")
		expected = flag.String("expected", "speech mesh", "substring expected in final transcript")
		chunkMS  = flag.Int("chunk-ms", 100, "audio chunk duration in milliseconds")
	)
	flag.Parse()

	if *wavPath == "" {
		exitf("--wav is required")
	}

	pcm, format, err := readWAV(*wavPath)
	if err != nil {
		exitf("read wav: %v", err)
	}
	if format.Encoding != speechmesh.AudioEncodingPCMS16LE {
		exitf("wav must be pcm_s16le, got %s", format.Encoding)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
	defer cancel()

	client, err := speechmesh.Dial(ctx, speechmesh.ClientConfig{URL: *url})
	if err != nil {
		exitf("dial speechmesh: %v", err)
	}
	defer client.Close()

	provider := speechmesh.DefaultProviderSelector()
	provider.RequiredCapabilities = []string{"streaming-input"}
	provider.PreferredCapabilities = []string{"on-device"}

	_, started, err := client.StartASR(ctx, speechmesh.StreamRequest{
		Provider:    provider,
		InputFormat: format,
		Options: speechmesh.RecognitionOptions{
			Language:       language,
			Hints:          []string{"speechmesh", "streaming", "go-sdk"},
			InterimResults: true,
			Punctuation:    true,
		},
	})
	if err != nil {
		exitf("start asr: %v", err)
	}
	fmt.Printf("session started provider=%s sample_rate=%d channels=%d\n", started.ProviderID, format.SampleRateHz, format.Channels)

	bytesPerChunk := int(format.SampleRateHz) * int(format.Channels) * 2 * (*chunkMS) / 1000
	if bytesPerChunk <= 0 {
		bytesPerChunk = len(pcm)
	}
	for start := 0; start < len(pcm); start += bytesPerChunk {
		end := start + bytesPerChunk
		if end > len(pcm) {
			end = len(pcm)
		}
		if err := client.SendAudio(ctx, pcm[start:end]); err != nil {
			exitf("send audio: %v", err)
		}
		time.Sleep(time.Duration(*chunkMS) * time.Millisecond)
	}

	if err := client.Commit(ctx); err != nil {
		exitf("commit: %v", err)
	}
	fmt.Println("audio stream committed, waiting for final transcript...")

	for {
		event, err := client.Recv(ctx)
		if err != nil {
			exitf("recv event: %v", err)
		}
		switch event.Type {
		case speechmesh.EventTypeASRResult:
			delta := ""
			if event.AsrResult.Delta != nil {
				delta = *event.AsrResult.Delta
			}
			fmt.Printf("result rev=%d final=%v speech_final=%v delta=%q text=%s\n",
				event.AsrResult.Revision,
				event.AsrResult.IsFinal,
				event.AsrResult.SpeechFinal,
				delta,
				event.AsrResult.Text,
			)
			if event.AsrResult.IsFinal && event.AsrResult.SpeechFinal {
				if !strings.Contains(strings.ToLower(event.AsrResult.Text), strings.ToLower(*expected)) {
					exitf("assertion failed: transcript does not contain %q; got=%q", *expected, event.AsrResult.Text)
				}
				fmt.Printf("final transcript: %s\n", event.AsrResult.Text)
				fmt.Println("assertion passed: transcript contains expected text")
				return
			}
		case speechmesh.EventTypeError:
			exitf("server error: %s", event.Error.Error.Message)
		}
	}
}

func readWAV(path string) ([]byte, speechmesh.AudioFormat, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, speechmesh.AudioFormat{}, err
	}
	if len(data) < 44 {
		return nil, speechmesh.AudioFormat{}, errors.New("wav too small")
	}
	if string(data[0:4]) != "RIFF" || string(data[8:12]) != "WAVE" {
		return nil, speechmesh.AudioFormat{}, errors.New("not a RIFF/WAVE file")
	}

	var (
		channels      uint16
		sampleRateHz  uint32
		bitsPerSample uint16
		pcm           []byte
	)
	reader := bytes.NewReader(data[12:])
	for reader.Len() >= 8 {
		var chunkID [4]byte
		var chunkSize uint32
		if err := binary.Read(reader, binary.LittleEndian, &chunkID); err != nil {
			return nil, speechmesh.AudioFormat{}, err
		}
		if err := binary.Read(reader, binary.LittleEndian, &chunkSize); err != nil {
			return nil, speechmesh.AudioFormat{}, err
		}
		if uint32(reader.Len()) < chunkSize {
			return nil, speechmesh.AudioFormat{}, errors.New("invalid wav chunk size")
		}
		chunk := make([]byte, chunkSize)
		if _, err := reader.Read(chunk); err != nil {
			return nil, speechmesh.AudioFormat{}, err
		}
		if chunkSize%2 == 1 && reader.Len() > 0 {
			_, _ = reader.ReadByte()
		}
		switch string(chunkID[:]) {
		case "fmt ":
			if len(chunk) < 16 {
				return nil, speechmesh.AudioFormat{}, errors.New("fmt chunk too small")
			}
			audioFormat := binary.LittleEndian.Uint16(chunk[0:2])
			channels = binary.LittleEndian.Uint16(chunk[2:4])
			sampleRateHz = binary.LittleEndian.Uint32(chunk[4:8])
			bitsPerSample = binary.LittleEndian.Uint16(chunk[14:16])
			if audioFormat != 1 {
				return nil, speechmesh.AudioFormat{}, fmt.Errorf("unsupported wav audio format: %d", audioFormat)
			}
		case "data":
			pcm = chunk
		}
	}

	if len(pcm) == 0 {
		return nil, speechmesh.AudioFormat{}, errors.New("wav missing data chunk")
	}
	if channels == 0 || sampleRateHz == 0 {
		return nil, speechmesh.AudioFormat{}, errors.New("wav missing fmt chunk")
	}
	if bitsPerSample != 16 {
		return nil, speechmesh.AudioFormat{}, fmt.Errorf("unsupported bits per sample: %d", bitsPerSample)
	}
	return pcm, speechmesh.PCMS16LE(sampleRateHz, channels), nil
}

func exitf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(1)
}

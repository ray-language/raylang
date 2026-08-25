// M133 — servidor gRPC REAL (google.golang.org/grpc, la implementación canónica) para el dogfood
// del cliente raylang (`grpc_client`). Codec crudo (bytes) + protobuf a mano: sin protoc.
//
// Servicio `greet.Greeter/Hello` (la MISMA ruta que usa examples/web/grpc_call_demo.ray, así el
// demo que valida el toy-server de tests/grpc_cli.rs valida también el servidor real): request
// campo 1 = name (string) → reply campo 1 = "hola, "+name. Con el argumento `unimplemented`
// registra OTRO servicio → la llamada del demo recibe UNIMPLEMENTED (12) en trailers-only.
//
// Uso: go run . <cert.pem> <key.pem> [unimplemented]  — imprime el puerto efímero por stdout.
package main

import (
	"context"
	"fmt"
	"net"
	"os"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials"
)

type bytesCodec struct{}

func (bytesCodec) Marshal(v interface{}) ([]byte, error)      { return *(v.(*[]byte)), nil }
func (bytesCodec) Unmarshal(data []byte, v interface{}) error { *(v.(*[]byte)) = data; return nil }
func (bytesCodec) Name() string                               { return "proto" }

// Campo 1 (len-delimited) de un mensaje protobuf.
func field1(msg []byte) string {
	if len(msg) < 2 || msg[0] != 0x0a {
		return ""
	}
	n := int(msg[1])
	if 2+n > len(msg) {
		return ""
	}
	return string(msg[2 : 2+n])
}

func hello(_ interface{}, _ context.Context, dec func(interface{}) error, _ grpc.UnaryServerInterceptor) (interface{}, error) {
	var in []byte
	if err := dec(&in); err != nil {
		return nil, err
	}
	reply := "hola, " + field1(in)
	out := append([]byte{0x0a, byte(len(reply))}, []byte(reply)...)
	return &out, nil
}

func main() {
	creds, err := credentials.NewServerTLSFromFile(os.Args[1], os.Args[2])
	if err != nil {
		panic(err)
	}
	s := grpc.NewServer(grpc.Creds(creds), grpc.ForceServerCodec(bytesCodec{}))
	service := "greet.Greeter"
	if len(os.Args) > 3 && os.Args[3] == "unimplemented" {
		service = "other.Nothing"
	}
	desc := grpc.ServiceDesc{
		ServiceName: service,
		HandlerType: (*interface{})(nil),
		Methods:     []grpc.MethodDesc{{MethodName: "Hello", Handler: hello}},
	}
	s.RegisterService(&desc, struct{}{})
	lis, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	fmt.Println(lis.Addr().(*net.TCPAddr).Port)
	os.Stdout.Sync()
	_ = s.Serve(lis)
}

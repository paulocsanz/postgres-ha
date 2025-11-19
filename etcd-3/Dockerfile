FROM quay.io/coreos/etcd:v3.5.16

EXPOSE 2379 2380

HEALTHCHECK --interval=10s --timeout=5s --start-period=10s \
  CMD etcdctl endpoint health || exit 1

CMD ["/usr/local/bin/etcd"]

FROM postgres:17-alpine

ARG PATRONI_VERSION=4.0.4

RUN apk add --no-cache \
    python3 \
    py3-pip \
    py3-psycopg2 \
    py3-yaml \
    curl \
    bash \
    && pip3 install --no-cache-dir --break-system-packages \
    patroni[etcd]==${PATRONI_VERSION}

RUN mkdir -p /home/postgres && \
    chown -R postgres:postgres /home/postgres /var/lib/postgresql

COPY patroni.yml /etc/patroni/patroni.yml
COPY docker-entrypoint.sh /docker-entrypoint.sh
RUN chmod +x /docker-entrypoint.sh

EXPOSE 5432 8008

HEALTHCHECK --interval=10s --timeout=5s --start-period=30s --retries=3 \
  CMD curl -f http://localhost:8008/health || exit 1

USER postgres

ENTRYPOINT ["/docker-entrypoint.sh"]
CMD ["patroni", "/etc/patroni/patroni.yml"]

FROM pgpool/pgpool:4.5-alpine

USER root

RUN apk add --no-cache curl bash

COPY pgpool.conf /etc/pgpool-II/pgpool.conf
COPY pool_hba.conf /etc/pgpool-II/pool_hba.conf
COPY docker-entrypoint.sh /docker-entrypoint.sh

RUN chmod +x /docker-entrypoint.sh && \
    chown -R postgres:postgres /etc/pgpool-II

USER postgres

EXPOSE 5432 9999

HEALTHCHECK --interval=10s --timeout=5s --start-period=30s \
  CMD pg_isready -h localhost -p 5432 || exit 1

ENTRYPOINT ["/docker-entrypoint.sh"]
CMD ["pgpool", "-n", "-f", "/etc/pgpool-II/pgpool.conf", "-F", "/etc/pgpool-II/pcp.conf"]
